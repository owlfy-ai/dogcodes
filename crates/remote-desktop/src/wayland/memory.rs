//! Per-plane MemFd mappings. DMA-BUFs never enter this CPU mapping boundary.
use crate::Result;
use pipewire::spa::{buffer::DataFlags, sys};
use std::{marker::PhantomData, ptr::NonNull};

pub(super) struct Mapping<'a> {
    address: NonNull<libc::c_void>,
    mapped: usize,
    delta: usize,
    length: usize,
    #[cfg(test)]
    writable: bool,
    _loan: PhantomData<&'a sys::spa_data>,
}
impl<'a> Mapping<'a> {
    pub fn new(data: &'a sys::spa_data, writable: bool) -> Result<Self> {
        if data.type_ != sys::SPA_DATA_MemFd
            || data.fd < 0
            || data.fd > i32::MAX as i64
            || data.maxsize == 0
            || data.maxsize > 512 * 1024 * 1024
            || data.flags & DataFlags::READABLE.bits() == 0
            || (writable && data.flags & DataFlags::WRITABLE.bits() == 0)
        {
            return Err("Wayland MemFd 平面无效或不可读取".into());
        }
        // SAFETY: fstat initializes this POD on success; sysconf needs no pointers.
        let (size, page) = unsafe {
            let mut stat: libc::stat = std::mem::zeroed();
            if libc::fstat(data.fd as i32, &mut stat) != 0 {
                return Err("无法读取 Wayland 缓冲区长度".into());
            }
            (stat.st_size, libc::sysconf(libc::_SC_PAGESIZE))
        };
        if page <= 0
            || size < 0
            || u64::from(data.mapoffset) + u64::from(data.maxsize) > size as u64
        {
            return Err("Wayland MemFd 平面超出文件边界".into());
        }
        let page = page as usize;
        let offset = data.mapoffset as usize / page * page;
        let delta = data.mapoffset as usize - offset;
        let mapped = delta
            .checked_add(data.maxsize as usize)
            .ok_or("Wayland 映射长度无效")?;
        let protection = libc::PROT_READ | if writable { libc::PROT_WRITE } else { 0 };
        // SAFETY: valid borrowed fd, page-aligned offset and checked file extent.
        // The mapping is released before the original PipeWire buffer loan ends.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                mapped,
                protection,
                libc::MAP_SHARED,
                data.fd as i32,
                offset as libc::off_t,
            )
        };
        if address == libc::MAP_FAILED {
            return Err("无法映射 Wayland 屏幕平面".into());
        }
        let Some(address) = NonNull::new(address) else {
            // SAFETY: even a valid zero-address mapping must be unmapped; no
            // Rust reference is constructed for it.
            unsafe {
                libc::munmap(address, mapped);
            }
            return Err("Wayland 屏幕映射地址无效".into());
        };
        Ok(Self {
            address,
            mapped,
            delta,
            length: data.maxsize as usize,
            #[cfg(test)]
            writable,
            _loan: PhantomData,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: checked mapping extent; the owning loan and mapping remain live.
        unsafe {
            std::slice::from_raw_parts(
                self.address.as_ptr().cast::<u8>().add(self.delta),
                self.length,
            )
        }
    }
    #[cfg(test)]
    pub fn bytes_mut(&mut self) -> Result<&mut [u8]> {
        if !self.writable {
            return Err("Wayland 映射为只读".into());
        }
        // SAFETY: writable mapping, exclusive Rust access for this synthetic
        // producer's loan, and no outstanding references to its bytes.
        Ok(unsafe {
            std::slice::from_raw_parts_mut(
                self.address.as_ptr().cast::<u8>().add(self.delta),
                self.length,
            )
        })
    }
}
impl Drop for Mapping<'_> {
    fn drop(&mut self) {
        // SAFETY: exactly the address and size returned by this owner's mmap.
        unsafe {
            libc::munmap(self.address.as_ptr(), self.mapped);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    #[test]
    fn unaligned_planes_map_independently_without_closing_the_original_fd() {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = tempfile::tempfile().unwrap();
        file.set_len(16384).unwrap();
        file.seek(SeekFrom::Start(4097)).unwrap();
        file.write_all(&[1, 2, 3, 4]).unwrap();
        let raw = sys::spa_data {
            type_: sys::SPA_DATA_MemFd,
            flags: DataFlags::READWRITE.bits(),
            fd: i64::from(file.as_raw_fd()),
            mapoffset: 4097,
            maxsize: 4,
            data: std::ptr::null_mut(),
            chunk: std::ptr::null_mut(),
        };
        let mut mapping = Mapping::new(&raw, false).unwrap();
        assert_eq!(mapping.bytes(), &[1, 2, 3, 4]);
        assert!(mapping.bytes_mut().is_err());
        drop(mapping);
        assert!(file.metadata().is_ok());
        let mut outside = raw;
        outside.maxsize = 16384;
        assert!(Mapping::new(&outside, false).is_err());
        let mut dma = raw;
        dma.type_ = sys::SPA_DATA_DmaBuf;
        assert!(Mapping::new(&dma, false).is_err());
    }
}
