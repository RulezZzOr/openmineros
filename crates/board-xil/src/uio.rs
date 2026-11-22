//! Read-write mapping of one Linux UIO device, found by its device-tree name.

use std::{
    fs::{self, File, OpenOptions},
    io,
    os::fd::AsRawFd,
    path::Path,
    ptr,
};

const SYS_UIO: &str = "/sys/class/uio";

pub struct UioDevice {
    base: *mut u32,
    len: usize,
    name: String,
    _file: File,
}

// The mapping is device memory owned by this value; access is by volatile
// word reads and writes only.
unsafe impl Send for UioDevice {}

impl UioDevice {
    /// Opens the UIO device whose `name` attribute equals `name`
    /// (for example `chain1-cmd-rx`) and maps its first region.
    pub fn open_by_name(name: &str) -> io::Result<Self> {
        let entry = find_by_name(Path::new(SYS_UIO), name)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no UIO device named {name}"),
            )
        })?;
        let size_path = Path::new(SYS_UIO).join(&entry).join("maps/map0/size");
        let len = parse_hex(&fs::read_to_string(&size_path)?).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bad size in {}", size_path.display()),
            )
        })?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(Path::new("/dev").join(&entry))?;

        // SAFETY: a fresh shared mapping of the device's first region (UIO
        // selects region N by offset N * page size, so 0 is map0).
        let base = unsafe {
            libc::mmap(
                ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            base: base.cast(),
            len,
            name: name.to_owned(),
            _file: file,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn check(&self, offset: usize) {
        assert!(
            offset % 4 == 0 && offset + 4 <= self.len,
            "offset {offset:#x} outside {} ({:#x} bytes)",
            self.name,
            self.len
        );
    }

    pub fn read(&self, offset: usize) -> u32 {
        self.check(offset);
        // SAFETY: aligned and inside the mapping (checked above).
        unsafe { ptr::read_volatile(self.base.add(offset / 4)) }
    }

    pub fn write(&self, offset: usize, value: u32) {
        self.check(offset);
        // SAFETY: aligned and inside the mapping (checked above).
        unsafe { ptr::write_volatile(self.base.add(offset / 4), value) }
    }
}

impl Drop for UioDevice {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly the mapping created in `open_by_name`.
        unsafe {
            libc::munmap(self.base.cast(), self.len);
        }
    }
}

/// Lists `(uioN, name)` pairs under `root`.
pub fn list(root: &Path) -> io::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let dir = entry.file_name().to_string_lossy().into_owned();
        if let Ok(name) = fs::read_to_string(entry.path().join("name")) {
            out.push((dir, name.trim().to_owned()));
        }
    }
    out.sort();
    Ok(out)
}

fn find_by_name(root: &Path, name: &str) -> io::Result<Option<String>> {
    Ok(list(root)?
        .into_iter()
        .find(|(_, n)| n == name)
        .map(|(d, _)| d))
}

fn parse_hex(text: &str) -> Option<usize> {
    let t = text.trim();
    usize::from_str_radix(t.strip_prefix("0x").unwrap_or(t), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sysfs_sizes() {
        assert_eq!(parse_hex("0x00001000\n"), Some(0x1000));
        assert_eq!(parse_hex("zz"), None);
    }

    #[test]
    fn finds_device_by_name() {
        let dir = tempfile::tempdir().unwrap();
        for (uio, name) in [("uio0", "chain1-common"), ("uio1", "chain1-cmd-rx")] {
            fs::create_dir(dir.path().join(uio)).unwrap();
            fs::write(dir.path().join(uio).join("name"), format!("{name}\n")).unwrap();
        }
        assert_eq!(
            find_by_name(dir.path(), "chain1-cmd-rx")
                .unwrap()
                .as_deref(),
            Some("uio1")
        );
        assert_eq!(find_by_name(dir.path(), "chain9-common").unwrap(), None);
    }
}
