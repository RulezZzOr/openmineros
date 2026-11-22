//! Read-only mapping of the FPGA register window.

use crate::fpga::{FPGA_BASE_ADDRESS, FPGA_WINDOW_LEN, Register, RegisterReader};
use std::{fs::File, io, os::fd::AsRawFd, ptr};

const PAGE_LEN: usize = 4096;

pub struct MappedRegisters {
    base: *const u32,
    source: &'static str,
}

impl MappedRegisters {
    /// Maps the window through the vendor character device when its kernel
    /// module is loaded, otherwise through `/dev/mem`.
    pub fn open() -> io::Result<Self> {
        Self::map("/dev/axi_fpga_dev", 0).or_else(|_| Self::map("/dev/mem", FPGA_BASE_ADDRESS))
    }

    fn map(path: &'static str, offset: u64) -> io::Result<Self> {
        // The file is opened read-only and mapped PROT_READ, so a write
        // through this mapping faults instead of reaching the hardware.
        let file = File::open(path)?;
        let offset = libc::off_t::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset out of range"))?;

        // SAFETY: a fresh shared read-only mapping of a device file; the
        // kernel picks the address and the length is one page.
        let base = unsafe {
            libc::mmap(
                ptr::null_mut(),
                PAGE_LEN,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                offset,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            base: base.cast(),
            source: path,
        })
    }

    /// Maps one page of `/dev/mem` at a caller-chosen physical base, read-only.
    /// Used to check which base a stock image answers on.
    pub fn open_at(base: u64) -> io::Result<Self> {
        Self::map("/dev/mem", base)
    }

    /// Reads one word at a byte offset inside the mapped page. Returns `None`
    /// for unaligned or out-of-page offsets instead of reading past the mapping.
    pub fn read_word(&self, offset: usize) -> Option<u32> {
        if offset % 4 != 0 || offset + 4 > PAGE_LEN {
            return None;
        }
        // SAFETY: the offset is 4-byte aligned and inside the mapped page.
        Some(unsafe { ptr::read_volatile(self.base.add(offset / 4)) })
    }

    pub fn source(&self) -> &'static str {
        self.source
    }
}

impl RegisterReader for MappedRegisters {
    fn read(&self, register: Register) -> u32 {
        let offset = register.offset();
        assert!(offset + 4 <= FPGA_WINDOW_LEN && FPGA_WINDOW_LEN <= PAGE_LEN);

        // SAFETY: the offset is 4-byte aligned and inside the mapped page.
        unsafe { ptr::read_volatile(self.base.add(offset / 4)) }
    }
}

impl Drop for MappedRegisters {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly the mapping created in `map`.
        unsafe {
            libc::munmap(self.base.cast_mut().cast(), PAGE_LEN);
        }
    }
}
