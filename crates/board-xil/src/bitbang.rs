//! Bit-banged I2C to the APW PSU through the 2-bit AXI GPIO at `0x41220000`
//! (gpio895 = SDA = bit 0, gpio896 = SCL = bit 1). bosminer maps the same
//! block through `/dev/mem`; while it is idle it leaves DATA = 3, TRI = 0.
//!
//! SCL and SDA are driven push-pull while we write; SDA is switched to input
//! (TRI = 1) only to sample ACK and data bits.

#[cfg(unix)]
pub use imp::BitBangI2c;
#[cfg(unix)]
pub use sniff::sniff;

/// Passive, read-only sampler of the PSU bus (a software logic analyzer).
/// It maps the GPIO block PROT_READ, so it cannot drive the lines.
#[cfg(unix)]
mod sniff {
    use std::{
        fs::File,
        io,
        os::fd::AsRawFd,
        ptr,
        time::{Duration, Instant},
    };

    /// Samples `DATA[1:0] | TRI[1:0] << 2` until `dur` passes and returns
    /// `(sample_index, value)` for every change, plus the total samples.
    /// `(change events as (sample index, value), total samples, elapsed)`.
    pub type Capture = (Vec<(u64, u8)>, u64, Duration);

    pub fn sniff(dur: Duration, cpu: usize) -> io::Result<Capture> {
        // SAFETY: plain sched_setaffinity on the calling thread.
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(cpu, &mut set);
            libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        }
        let f = File::open("/dev/mem")?;
        // SAFETY: read-only shared mapping of one page of device memory.
        let p = unsafe {
            libc::mmap(
                ptr::null_mut(),
                4096,
                libc::PROT_READ,
                libc::MAP_SHARED,
                f.as_raw_fd(),
                0x4122_0000,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let regs = p as *const u32;
        let mut out = Vec::with_capacity(1 << 20);
        let start = Instant::now();
        let mut last = 0xFFu8;
        let mut n: u64 = 0;
        loop {
            // SAFETY: words 0 and 1 of the mapped page.
            let v = unsafe {
                (ptr::read_volatile(regs) & 3) | ((ptr::read_volatile(regs.add(1)) & 3) << 2)
            } as u8;
            if v != last {
                out.push((n, v));
                last = v;
            }
            n += 1;
            if n & 0xFFFF == 0 && start.elapsed() >= dur {
                break;
            }
        }
        let el = start.elapsed();
        // SAFETY: unmaps the mapping above.
        unsafe { libc::munmap(p, 4096) };
        Ok((out, n, el))
    }
}

#[cfg(unix)]
mod imp {
    use std::{fs::OpenOptions, io, os::fd::AsRawFd, ptr, time::Duration, time::Instant};

    const BASE: libc::off_t = 0x4122_0000;
    const DATA: usize = 0;
    const TRI: usize = 1;
    const SDA: u32 = 1 << 0;
    const SCL: u32 = 1 << 1;

    pub struct BitBangI2c {
        regs: *mut u32,
        half: Duration,
    }

    impl BitBangI2c {
        pub fn open(half_period_us: u64) -> io::Result<Self> {
            let f = OpenOptions::new().read(true).write(true).open("/dev/mem")?;
            // SAFETY: one shared page of device memory at the AXI GPIO base.
            let p = unsafe {
                libc::mmap(
                    ptr::null_mut(),
                    4096,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    f.as_raw_fd(),
                    BASE,
                )
            };
            if p == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            let me = Self {
                regs: p.cast(),
                half: Duration::from_micros(half_period_us),
            };
            me.set(DATA, SDA | SCL);
            me.set(TRI, 0);
            Ok(me)
        }

        fn set(&self, reg: usize, v: u32) {
            // SAFETY: DATA and TRI are the first two words of the mapped page.
            unsafe { ptr::write_volatile(self.regs.add(reg), v) }
        }

        fn get(&self, reg: usize) -> u32 {
            // SAFETY: as above.
            unsafe { ptr::read_volatile(self.regs.add(reg)) }
        }

        fn wait(&self) {
            let t = Instant::now() + self.half;
            while Instant::now() < t {
                std::hint::spin_loop();
            }
        }

        /// Drives SDA and SCL as outputs with the given levels.
        fn drive(&self, sda: bool, scl: bool) {
            self.set(
                DATA,
                (if sda { SDA } else { 0 }) | (if scl { SCL } else { 0 }),
            );
            self.set(TRI, 0);
        }

        fn start(&self) {
            self.drive(true, true);
            self.wait();
            self.drive(false, true);
            self.wait();
            self.drive(false, false);
            self.wait();
        }

        fn stop(&self) {
            self.drive(false, false);
            self.wait();
            self.drive(false, true);
            self.wait();
            self.drive(true, true);
            self.wait();
        }

        // Bit cell as bosminer drives it (measured): SCL low, wait T, change
        // SDA, wait T, SCL high for T, SCL low. Changing SDA right at the
        // falling SCL edge made the PSU's software I2C slave NAK.
        fn write_bit(&self, bit: bool) {
            self.drive(false, false);
            self.wait();
            self.drive(bit, false);
            self.wait();
            self.drive(bit, true);
            self.wait();
            self.drive(bit, false);
        }

        /// Releases SDA, clocks one bit and samples it at the end of SCL high.
        fn read_bit(&self) -> bool {
            self.set(DATA, 0);
            self.set(TRI, 0);
            self.wait();
            self.set(TRI, SDA);
            self.wait();
            self.set(DATA, SCL);
            self.wait();
            let b = self.get(DATA) & SDA != 0;
            self.set(DATA, 0);
            b
        }

        /// Sends a byte, returns true on ACK.
        fn write_byte(&self, byte: u8) -> bool {
            for i in (0..8).rev() {
                self.write_bit(byte >> i & 1 == 1);
            }
            let ack = !self.read_bit();
            self.drive(false, false);
            ack
        }

        fn read_byte(&self, ack: bool) -> u8 {
            let mut b = 0u8;
            for _ in 0..8 {
                b = (b << 1) | u8::from(self.read_bit());
            }
            self.write_bit(!ack);
            b
        }

        /// Address-only probe (write direction). Returns true if ACKed.
        pub fn probe(&self, addr: u8) -> bool {
            self.start();
            let ack = self.write_byte(addr << 1);
            self.stop();
            ack
        }

        /// One write transaction. Returns the index of the first NAKed byte.
        pub fn write(&self, addr: u8, bytes: &[u8]) -> Result<(), String> {
            self.start();
            if !self.write_byte(addr << 1) {
                self.stop();
                return Err(format!("NAK on address {addr:#04x}"));
            }
            for (i, b) in bytes.iter().enumerate() {
                if !self.write_byte(*b) {
                    self.stop();
                    return Err(format!("NAK on byte {i} ({b:#04x})"));
                }
            }
            self.stop();
            Ok(())
        }

        /// One single-byte read transaction (NAK, as bosminer does).
        fn read_one(&self, addr: u8) -> Result<u8, String> {
            self.start();
            if !self.write_byte((addr << 1) | 1) {
                self.stop();
                return Err(format!("NAK on read address {addr:#04x}"));
            }
            let b = self.read_byte(false);
            self.stop();
            Ok(b)
        }

        /// APW request/reply exactly as bosminer does it on the wire:
        /// each request byte as `[addr_W] [0x11] [byte]` with ~60 ms gaps,
        /// ~400 ms wait, then the reply one byte per read transaction.
        pub fn apw_exchange(&self, addr: u8, frame: &[u8]) -> Result<Vec<u8>, String> {
            for (i, b) in frame.iter().enumerate() {
                self.write(addr, &[crate::i2c::apw::WRITE_REG, *b])
                    .map_err(|e| format!("tx byte {i}: {e}"))?;
                std::thread::sleep(Duration::from_millis(60));
            }
            std::thread::sleep(Duration::from_millis(400));
            let mut out = Vec::new();
            for _ in 0..3 {
                out.push(self.read_one(addr)?);
                std::thread::sleep(Duration::from_millis(30));
            }
            if out[..2] == [0x55, 0xAA] && (4..=64).contains(&out[2]) {
                for _ in 0..usize::from(out[2]) - 1 {
                    out.push(self.read_one(addr)?);
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
            Ok(out)
        }

        /// APW framed reply: reads the 3-byte header, then `len - 1` more.
        pub fn read_frame(&self, addr: u8) -> Result<Vec<u8>, String> {
            self.start();
            if !self.write_byte((addr << 1) | 1) {
                self.stop();
                return Err(format!("NAK on read address {addr:#04x}"));
            }
            let mut out = vec![self.read_byte(true), self.read_byte(true)];
            let ok = out == [0x55, 0xAA];
            let len = self.read_byte(ok);
            out.push(len);
            if ok && (2..64).contains(&len) {
                for i in 0..usize::from(len) - 1 {
                    let last = i + 2 == usize::from(len);
                    out.push(self.read_byte(!last));
                }
            }
            self.stop();
            Ok(out)
        }
    }

    impl Drop for BitBangI2c {
        fn drop(&mut self) {
            // Leave the lines as bosminer keeps them when idle.
            self.set(DATA, SDA | SCL);
            self.set(TRI, 0);
            // SAFETY: unmaps the page mapped in `open`.
            unsafe {
                libc::munmap(self.regs.cast(), 4096);
            }
        }
    }
}
