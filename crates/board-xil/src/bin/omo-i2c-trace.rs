//! Records the I2C traffic of a program it starts (research tool, ARM only).
//!
//!   omo-i2c-trace <seconds> <program> [args…]
//!
//! The program runs under ptrace from its first instruction; the tracer logs
//! open/ioctl/read/write on `/dev/i2c-*` with the data bytes to stderr, then
//! sends SIGTERM after <seconds> and keeps tracing until it exits. Nothing is
//! written to any device by the tracer itself.

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn main() {
    std::process::exit(imp::run());
}

#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
fn main() {
    eprintln!("omo-i2c-trace supports linux/arm only");
    std::process::exit(2);
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
mod imp {
    use std::{
        collections::{HashMap, HashSet},
        ffi::CString,
        time::{Duration, Instant},
    };

    const SYS_READ: u32 = 3;
    const SYS_WRITE: u32 = 4;
    const SYS_OPEN: u32 = 5;
    const SYS_CLOSE: u32 = 6;
    const SYS_IOCTL: u32 = 54;
    const SYS_OPENAT: u32 = 322;
    const I2C_SLAVE: u32 = 0x0703;
    const I2C_SLAVE_FORCE: u32 = 0x0706;
    const I2C_RDWR: u32 = 0x0707;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Regs {
        r: [u32; 18],
    }

    fn regs(pid: i32) -> Regs {
        let mut r = Regs::default();
        // SAFETY: PTRACE_GETREGS fills a struct pt_regs (18 words on ARM).
        unsafe { libc::ptrace(libc::PTRACE_GETREGS, pid, 0, &mut r as *mut Regs) };
        r
    }

    fn peek(pid: i32, addr: u32, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let local = libc::iovec {
            iov_base: out.as_mut_ptr().cast(),
            iov_len: len,
        };
        let remote = libc::iovec {
            iov_base: addr as usize as *mut libc::c_void,
            iov_len: len,
        };
        // SAFETY: reads the tracee's memory into our buffer.
        let n = unsafe { libc::process_vm_readv(pid, &local, 1, &remote, 1, 0) };
        out.truncate(n.max(0) as usize);
        out
    }

    fn cstr(pid: i32, addr: u32) -> String {
        let b = peek(pid, addr, 64);
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        String::from_utf8_lossy(&b[..end]).into_owned()
    }

    fn hex(b: &[u8]) -> String {
        b.iter()
            .map(|x| format!("{x:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn run() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.len() < 2 {
            eprintln!("usage: omo-i2c-trace <seconds> <program> [args...]");
            return 2;
        }
        let secs: u64 = args[0].parse().unwrap_or(60);
        let prog = CString::new(args[1].clone()).unwrap();
        let cargs: Vec<CString> = args[1..]
            .iter()
            .map(|a| CString::new(a.as_str()).unwrap())
            .collect();
        let mut argv: Vec<*const libc::c_char> = cargs.iter().map(|a| a.as_ptr()).collect();
        argv.push(std::ptr::null());

        // SAFETY: classic fork + PTRACE_TRACEME + execv.
        let child = unsafe { libc::fork() };
        if child == 0 {
            unsafe {
                libc::ptrace(libc::PTRACE_TRACEME, 0, 0, 0);
                libc::raise(libc::SIGSTOP);
                libc::execv(prog.as_ptr(), argv.as_ptr());
                libc::_exit(127);
            }
        }
        let mut status = 0;
        // SAFETY: wait for the initial SIGSTOP of the child.
        unsafe { libc::waitpid(child, &mut status, 0) };
        let opts = libc::PTRACE_O_TRACESYSGOOD
            | libc::PTRACE_O_TRACECLONE
            | libc::PTRACE_O_TRACEFORK
            | libc::PTRACE_O_TRACEVFORK;
        unsafe {
            libc::ptrace(libc::PTRACE_SETOPTIONS, child, 0, opts);
            libc::ptrace(libc::PTRACE_SYSCALL, child, 0, 0);
        }

        let start = Instant::now();
        let mut termed = false;
        let mut in_call: HashMap<i32, Regs> = HashMap::new();
        let mut i2c_fds: HashMap<(i32, i32), String> = HashMap::new(); // (tgid-agnostic) fd -> path
        let mut pending_open: HashMap<i32, String> = HashMap::new();
        let mut live: HashSet<i32> = HashSet::from([child]);
        let mut slave: HashMap<i32, u32> = HashMap::new();
        let ts = |s: &Instant| format!("{:9.3}", s.elapsed().as_secs_f64());

        while !live.is_empty() {
            if !termed && start.elapsed() > Duration::from_secs(secs) {
                unsafe { libc::kill(child, libc::SIGTERM) };
                termed = true;
                eprintln!("{} -- SIGTERM sent", ts(&start));
            }
            // SAFETY: wait for any traced thread (__WALL).
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::__WALL) };
            if pid < 0 {
                break;
            }
            if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                live.remove(&pid);
                in_call.remove(&pid);
                continue;
            }
            live.insert(pid);
            let mut deliver = 0;
            if libc::WIFSTOPPED(status) {
                let sig = libc::WSTOPSIG(status);
                if sig == (libc::SIGTRAP | 0x80) {
                    if let Some(entry) = in_call.remove(&pid) {
                        // syscall exit
                        let exit = regs(pid);
                        let nr = entry.r[7];
                        let ret = exit.r[0] as i32;
                        let fd = entry.r[17] as i32; // ORIG_r0
                        match nr {
                            SYS_OPEN | SYS_OPENAT => {
                                if let Some(path) = pending_open.remove(&pid) {
                                    if ret >= 0 {
                                        i2c_fds.insert((0, ret), path.clone());
                                        eprintln!("{} open {path} -> fd {ret}", ts(&start));
                                    }
                                }
                            }
                            SYS_READ if i2c_fds.contains_key(&(0, fd)) => {
                                let data = if ret > 0 {
                                    peek(pid, entry.r[1], ret as usize)
                                } else {
                                    vec![]
                                };
                                eprintln!(
                                    "{} R  {:#04x} ret={ret} [{}]",
                                    ts(&start),
                                    slave.get(&fd).copied().unwrap_or(0),
                                    hex(&data)
                                );
                            }
                            SYS_WRITE if i2c_fds.contains_key(&(0, fd)) => {
                                let data = peek(pid, entry.r[1], entry.r[2] as usize);
                                eprintln!(
                                    "{} W  {:#04x} ret={ret} [{}]",
                                    ts(&start),
                                    slave.get(&fd).copied().unwrap_or(0),
                                    hex(&data)
                                );
                            }
                            SYS_IOCTL if i2c_fds.contains_key(&(0, fd)) => {
                                let req = entry.r[1];
                                if req == I2C_SLAVE || req == I2C_SLAVE_FORCE {
                                    slave.insert(fd, entry.r[2]);
                                } else if req == I2C_RDWR {
                                    let hdr = peek(pid, entry.r[2], 8);
                                    if hdr.len() == 8 {
                                        let msgs =
                                            u32::from_le_bytes(hdr[0..4].try_into().unwrap());
                                        let n = u32::from_le_bytes(hdr[4..8].try_into().unwrap())
                                            .min(8);
                                        let mut parts = Vec::new();
                                        for i in 0..n {
                                            let m = peek(pid, msgs + i * 12, 12);
                                            if m.len() < 12 {
                                                break;
                                            }
                                            let addr = u16::from_le_bytes([m[0], m[1]]);
                                            let flags = u16::from_le_bytes([m[2], m[3]]);
                                            let len = u16::from_le_bytes([m[4], m[5]]) as usize;
                                            let buf =
                                                u32::from_le_bytes(m[8..12].try_into().unwrap());
                                            let dir = if flags & 1 != 0 { "R" } else { "W" };
                                            parts.push(format!(
                                                "{dir}{addr:#04x}[{}]",
                                                hex(&peek(pid, buf, len.min(64)))
                                            ));
                                        }
                                        eprintln!(
                                            "{} RDWR ret={ret} {}",
                                            ts(&start),
                                            parts.join(" ")
                                        );
                                    }
                                } else {
                                    eprintln!(
                                        "{} ioctl fd={fd} req={req:#x} arg={:#x} ret={ret}",
                                        ts(&start),
                                        entry.r[2]
                                    );
                                }
                            }
                            SYS_CLOSE => {
                                i2c_fds.remove(&(0, fd));
                            }
                            _ => {}
                        }
                    } else {
                        // syscall entry
                        let r = regs(pid);
                        let nr = r.r[7];
                        if nr == SYS_OPEN || nr == SYS_OPENAT {
                            let path = cstr(pid, if nr == SYS_OPEN { r.r[0] } else { r.r[1] });
                            if path.starts_with("/dev/i2c") {
                                pending_open.insert(pid, path);
                            }
                        }
                        in_call.insert(pid, r);
                    }
                } else if sig == libc::SIGTRAP {
                    // ptrace event (clone/fork): nothing to deliver
                } else if sig == libc::SIGSTOP && !in_call.contains_key(&pid) {
                    // new thread's initial stop
                } else {
                    deliver = sig;
                }
            }
            // SAFETY: resume the thread until its next syscall boundary.
            unsafe { libc::ptrace(libc::PTRACE_SYSCALL, pid, 0, deliver) };
        }
        eprintln!("{} -- traced program exited", ts(&start));
        0
    }
}
