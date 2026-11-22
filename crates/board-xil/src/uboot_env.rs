//! Redundant U-Boot environment on the X19 control board: two copies of
//! 0x20000 bytes in mtd4 ("uboot_env") at offsets 0 and 0x20000, each
//! `crc32(LE) | flag | "k=v\0…\0\0" padding` (crc over everything after the
//! flag byte). Writes always go to the inactive copy with flag + 1, so a
//! failed write leaves the previous environment in force.

pub const COPY_SIZE: usize = 0x20000;
pub const OFFSETS: [u64; 2] = [0, 0x20000];
pub const MTD: &str = "/dev/mtd4";

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[derive(Debug, Clone, PartialEq)]
pub struct Env {
    pub vars: Vec<(String, String)>,
}

impl Env {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    }

    /// Sets `k` (empty value deletes it, as `fw_setenv k` does).
    pub fn set(&mut self, k: &str, v: &str) {
        self.vars.retain(|(n, _)| n != k);
        if !v.is_empty() {
            self.vars.push((k.to_owned(), v.to_owned()));
        }
    }
}

/// Parses one copy; returns (env, flag) if the CRC matches.
pub fn parse_copy(raw: &[u8]) -> Option<(Env, u8)> {
    if raw.len() != COPY_SIZE {
        return None;
    }
    let crc = u32::from_le_bytes(raw[0..4].try_into().ok()?);
    let data = &raw[5..];
    if crc32(data) != crc {
        return None;
    }
    let mut vars = Vec::new();
    for item in data.split(|&b| b == 0) {
        if item.is_empty() {
            break;
        }
        let s = String::from_utf8_lossy(item);
        if let Some((k, v)) = s.split_once('=') {
            vars.push((k.to_owned(), v.to_owned()));
        }
    }
    Some((Env { vars }, raw[4]))
}

pub fn serialize(env: &Env, flag: u8) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(COPY_SIZE - 5);
    for (k, v) in &env.vars {
        data.extend_from_slice(k.as_bytes());
        data.push(b'=');
        data.extend_from_slice(v.as_bytes());
        data.push(0);
    }
    data.push(0);
    if data.len() > COPY_SIZE - 5 {
        return Err("environment too large".into());
    }
    data.resize(COPY_SIZE - 5, 0);
    let mut out = crc32(&data).to_le_bytes().to_vec();
    out.push(flag);
    out.extend(data);
    Ok(out)
}

/// Index of the active copy, as U-Boot chooses it.
/// Both copies: parsed env and flag, or None when the CRC fails.
pub type Copies = [Option<(Env, u8)>; 2];

pub fn active(copies: &Copies) -> Option<usize> {
    match (&copies[0], &copies[1]) {
        (Some(_), None) => Some(0),
        (None, Some(_)) => Some(1),
        (None, None) => None,
        (Some((_, f0)), Some((_, f1))) => Some(if f0 == f1 {
            0
        } else if *f0 == 0xFF && *f1 == 0 {
            1
        } else if *f1 == 0xFF && *f0 == 0 {
            0
        } else if f1 > f0 {
            1
        } else {
            0
        }),
    }
}

#[cfg(unix)]
pub mod hw {
    use super::*;
    use std::{fs::OpenOptions, os::fd::AsRawFd, os::unix::fs::FileExt};

    #[repr(C)]
    struct EraseInfo {
        start: u32,
        length: u32,
    }
    const MEMERASE: libc::c_ulong = 0x4008_4D02;

    pub fn read() -> Result<(Copies, usize), String> {
        let f = OpenOptions::new()
            .read(true)
            .open(MTD)
            .map_err(|e| format!("{MTD}: {e}"))?;
        let mut copies: Copies = [None, None];
        for (i, off) in OFFSETS.iter().enumerate() {
            let mut raw = vec![0u8; COPY_SIZE];
            f.read_exact_at(&mut raw, *off).map_err(|e| e.to_string())?;
            copies[i] = parse_copy(&raw);
        }
        let a = active(&copies).ok_or("no valid U-Boot environment copy")?;
        Ok((copies, a))
    }

    /// Applies `changes` to the active environment and writes the result to
    /// the inactive copy; verifies by reading back.
    pub fn update(changes: &[(&str, &str)]) -> Result<Env, String> {
        let (copies, a) = read()?;
        let (env, flag) = copies[a].clone().ok_or("no active copy")?;
        let mut env = env;
        for (k, v) in changes {
            env.set(k, v);
        }
        let target = 1 - a;
        let raw = serialize(&env, flag.wrapping_add(1))?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(MTD)
            .map_err(|e| format!("{MTD}: {e}"))?;
        let ei = EraseInfo {
            start: OFFSETS[target] as u32,
            length: COPY_SIZE as u32,
        };
        // SAFETY: MEMERASE with a properly laid out erase_info_user.
        if unsafe { libc::ioctl(f.as_raw_fd(), MEMERASE as _, &ei) } < 0 {
            return Err(format!("erase: {}", std::io::Error::last_os_error()));
        }
        f.write_all_at(&raw, OFFSETS[target])
            .map_err(|e| format!("write: {e}"))?;
        let mut back = vec![0u8; COPY_SIZE];
        f.read_exact_at(&mut back, OFFSETS[target])
            .map_err(|e| e.to_string())?;
        if back != raw {
            return Err("read-back mismatch; the previous copy stays active".into());
        }
        let (now, na) = read()?;
        if na != target || now[target].as_ref().map(|c| &c.0) != Some(&env) {
            return Err("new copy is not the active one after write".into());
        }
        Ok(env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_zlib() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn roundtrip_and_active_choice() {
        let env = Env {
            vars: vec![
                ("firmware".into(), "2".into()),
                ("upgrade_stage".into(), "1".into()),
            ],
        };
        let raw = serialize(&env, 7).unwrap();
        let (back, flag) = parse_copy(&raw).unwrap();
        assert_eq!((back.clone(), flag), (env.clone(), 7));
        let mut e2 = back;
        e2.set("upgrade_stage", "");
        assert_eq!(e2.get("upgrade_stage"), None);
        assert_eq!(e2.get("firmware"), Some("2"));
        let c = [Some((env.clone(), 7)), Some((e2.clone(), 8))];
        assert_eq!(active(&c), Some(1));
        let c = [Some((env.clone(), 0)), Some((e2, 0xFF))];
        assert_eq!(active(&c), Some(0));
        let mut bad = raw;
        bad[100] ^= 1;
        assert!(parse_copy(&bad).is_none());
    }
}
