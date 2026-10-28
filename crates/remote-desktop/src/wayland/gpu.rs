//! DMA-BUF descriptors and device-local offscreen import. No desktop connection.
mod egl;
use super::pixels::{Matrix, Range};
use crate::Result;
use image::RgbaImage;
use pipewire::spa::{param::video::VideoFormat, sys};
use std::os::fd::{AsRawFd, BorrowedFd};
pub(super) const INVALID_MODIFIER: u64 = (1u64 << 56) - 1;

pub(super) struct Plane<'a> {
    pub fd: BorrowedFd<'a>,
    pub offset: u32,
    pub pitch: i32,
}
pub(super) struct Frame<'a> {
    pub size: (u32, u32),
    pub format: VideoFormat,
    pub modifier: Option<u64>,
    pub colors: Option<(Matrix, Range)>,
    pub planes: Vec<Plane<'a>>,
}
pub(super) struct Capability {
    pub format: VideoFormat,
    pub modifiers: Vec<u64>,
    pub max_size: u32,
}
pub(super) struct Importer {
    devices: Vec<egl::Device>,
    rejected: Vec<(VideoFormat, u64)>,
}
impl Importer {
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            devices: Vec::new(),
            rejected: Vec::new(),
        }
    }
    pub fn discover() -> Self {
        Self {
            devices: egl::devices().unwrap_or_default(),
            rejected: Vec::new(),
        }
    }
    pub fn capabilities(&self) -> Vec<Capability> {
        if self.rejected.len() >= 16 {
            return Vec::new();
        }
        self.devices
            .iter()
            .flat_map(|device| device.capabilities())
            .map(|mut cap| {
                cap.modifiers
                    .retain(|modifier| !self.rejected.contains(&(cap.format, *modifier)));
                cap
            })
            .filter(|cap| !cap.modifiers.is_empty())
            .collect()
    }
    pub fn supports(&self, format: VideoFormat, modifier: u64, size: (u32, u32)) -> bool {
        self.capabilities().iter().any(|cap| {
            cap.format == format
                && cap.modifiers.contains(&modifier)
                && size.0 <= cap.max_size
                && size.1 <= cap.max_size
        })
    }
    pub fn reject(&mut self, format: VideoFormat, modifier: u64) {
        if !self.rejected.contains(&(format, modifier)) && self.rejected.len() < 16 {
            self.rejected.push((format, modifier));
        }
    }
    pub fn read(&mut self, frame: &Frame<'_>) -> Result<RgbaImage> {
        let modifier = frame
            .modifier
            .ok_or("Wayland GPU 来源未提供格式 modifier")?;
        let attrs = attributes(frame)?;
        for device in &mut self.devices {
            if device.supports(frame.format, modifier, frame.size)
                && let Ok(image) = device.read(frame.size, &attrs)
            {
                return Ok(image);
            }
        }
        Err("显卡无法读取当前屏幕共享格式".into())
    }
}

pub(super) const FORMATS: [(VideoFormat, u32); 11] = [
    (VideoFormat::BGRA, u32::from_le_bytes(*b"AR24")),
    (VideoFormat::BGRx, u32::from_le_bytes(*b"XR24")),
    (VideoFormat::RGBA, u32::from_le_bytes(*b"AB24")),
    (VideoFormat::RGBx, u32::from_le_bytes(*b"XB24")),
    (VideoFormat::ARGB, u32::from_le_bytes(*b"BA24")),
    (VideoFormat::ABGR, u32::from_le_bytes(*b"RA24")),
    (VideoFormat::xRGB, u32::from_le_bytes(*b"BX24")),
    (VideoFormat::xBGR, u32::from_le_bytes(*b"RX24")),
    (VideoFormat::RGB, u32::from_le_bytes(*b"BG24")),
    (VideoFormat::BGR, u32::from_le_bytes(*b"RG24")),
    (VideoFormat::NV12, u32::from_le_bytes(*b"NV12")),
];

fn attributes(frame: &Frame<'_>) -> Result<Vec<i32>> {
    let (w, h) = frame.size;
    if w == 0
        || h == 0
        || w > 16384
        || h > 16384
        || u64::from(w) * u64::from(h) > 64 * 1024 * 1024
        || frame.planes.is_empty()
        || frame.planes.len() > 4
    {
        return Err("Wayland GPU 画面尺寸或平面数量无效".into());
    }
    let fourcc = FORMATS
        .iter()
        .find(|(format, _)| *format == frame.format)
        .ok_or("Wayland GPU 像素格式不支持")?
        .1;
    let mut attrs = vec![0x3057, w as i32, 0x3056, h as i32, 0x3271, fourcc as i32];
    for (i, plane) in frame.planes.iter().enumerate() {
        if plane.pitch <= 0 || plane.offset > i32::MAX as u32 {
            return Err("Wayland GPU 平面偏移或行跨度无效".into());
        }
        let base = if i == 3 {
            0x3440
        } else {
            0x3272 + 3 * i as i32
        };
        attrs.extend_from_slice(&[
            base,
            plane.fd.as_raw_fd(),
            base + 1,
            plane.offset as i32,
            base + 2,
            plane.pitch,
        ]);
        if let Some(modifier) = frame.modifier.filter(|value| *value != INVALID_MODIFIER) {
            attrs.extend_from_slice(&[
                0x3443 + 2 * i as i32,
                modifier as i32,
                0x3444 + 2 * i as i32,
                (modifier >> 32) as i32,
            ]);
        }
    }
    if frame.format == VideoFormat::NV12 {
        if !w.is_multiple_of(2) || !h.is_multiple_of(2) || frame.planes.len() < 2 {
            return Err("Wayland GPU NV12 平面布局无效".into());
        }
        let (matrix, range) = frame.colors.ok_or("Wayland GPU NV12 色彩信息缺失")?;
        attrs.extend_from_slice(&[
            0x327B,
            match matrix {
                Matrix::Bt601 => 0x327F,
                Matrix::Bt709 => 0x3280,
            },
            0x327C,
            match range {
                Range::Full => 0x3282,
                Range::Limited => 0x3283,
            },
        ]);
    }
    attrs.push(0x3038);
    Ok(attrs)
}

/// A DMA-BUF's maxsize/chunk size may legally be zero. Never mmap it or use
/// SHM's modulo offsets; these offsets are relative to the file descriptor.
pub(super) fn planes(data: &[pipewire::spa::buffer::Data]) -> Result<Option<Vec<Plane<'_>>>> {
    if data.is_empty() || data.len() > 4 {
        return Err("Wayland GPU 平面数量不支持".into());
    }
    let mut planes = Vec::with_capacity(data.len());
    for data in data {
        let raw = data.as_raw();
        if raw.type_ != sys::SPA_DATA_DmaBuf
            || raw.fd < 0
            || raw.fd > i32::MAX as i64
            || raw.chunk.is_null()
        {
            return Err("Wayland GPU 平面或同步方式不支持".into());
        }
        // SAFETY: the dequeued PipeWire buffer keeps its chunk and borrowed fd
        // valid until the synchronous import/readback and its image cleanup finish.
        let chunk = unsafe { &*raw.chunk };
        if chunk.flags & pipewire::spa::buffer::ChunkFlags::CORRUPTED.bits() != 0 {
            return Ok(None);
        }
        let offset = raw
            .mapoffset
            .checked_add(chunk.offset)
            .ok_or("Wayland GPU 平面偏移溢出")?;
        // SAFETY: PipeWire retains ownership of this fd throughout the buffer loan.
        let fd = unsafe { BorrowedFd::borrow_raw(raw.fd as i32) };
        planes.push(Plane {
            fd,
            offset,
            pitch: chunk.stride,
        });
    }
    Ok(Some(planes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auxiliary_planes_modifiers_and_yuv_hints_are_not_lost() {
        let file = std::fs::File::open("/dev/null").unwrap();
        use std::os::fd::AsFd;
        let mut frame = Frame {
            size: (2, 2),
            format: VideoFormat::NV12,
            modifier: Some(0x1234_5678_9abc_def0),
            colors: Some((Matrix::Bt709, Range::Full)),
            planes: (0..4)
                .map(|i| Plane {
                    fd: file.as_fd(),
                    offset: 32 + i * 64,
                    pitch: 336,
                })
                .collect(),
        };
        let attrs = attributes(&frame).unwrap();
        assert!(attrs.windows(2).any(|v| v == [0x3440, file.as_raw_fd()]));
        assert!(
            attrs
                .windows(2)
                .any(|v| v == [0x3449, 0x9abc_def0u32 as i32])
        );
        assert!(attrs.windows(2).any(|v| v == [0x327B, 0x3280]));
        frame.planes[0].offset = u32::MAX;
        assert!(attributes(&frame).is_err());
        frame.planes[0].offset = 0;
        frame.planes[0].pitch = -320;
        assert!(attributes(&frame).is_err());
    }

    #[test]
    fn implicit_modifier_is_56_bit_invalid_and_does_not_become_an_explicit_layout() {
        use std::os::fd::AsFd;
        let file = std::fs::File::open("/dev/null").unwrap();
        let frame = Frame {
            size: (2, 2),
            format: VideoFormat::BGRx,
            modifier: Some((1u64 << 56) - 1),
            colors: None,
            planes: vec![Plane {
                fd: file.as_fd(),
                offset: 0,
                pitch: 8,
            }],
        };
        assert!(
            !attributes(&frame)
                .unwrap()
                .chunks_exact(2)
                .any(|v| v[0] == 0x3443)
        );
    }

    #[test]
    fn zero_sized_dma_chunks_use_fd_offsets_and_keep_descriptor_ownership() {
        let file = std::fs::File::open("/dev/null").unwrap();
        let mut chunk = sys::spa_chunk {
            offset: 32,
            stride: 336,
            size: 0,
            flags: 0,
        };
        let mut raw = sys::spa_data {
            type_: sys::SPA_DATA_DmaBuf,
            flags: 0,
            fd: i64::from(file.as_raw_fd()),
            mapoffset: 4096,
            maxsize: 0,
            data: std::ptr::null_mut(),
            chunk: &mut chunk,
        };
        fn read(raw: &sys::spa_data) -> Result<Option<Vec<Plane<'_>>>> {
            // SAFETY: Data is repr(transparent); descriptor/chunk/fd all stay
            // alive for this synchronous parse. No fd import or CPU read occurs.
            let data = unsafe { std::slice::from_raw_parts(std::ptr::from_ref(raw).cast(), 1) };
            planes(data)
        }
        let parsed = read(&raw).unwrap().unwrap();
        assert_eq!(parsed[0].offset, 4128);
        assert_eq!(parsed[0].pitch, 336);
        drop(parsed);
        assert!(file.metadata().is_ok());
        raw.mapoffset = u32::MAX;
        assert!(read(&raw).is_err());
        raw.mapoffset = 0;
        chunk.flags = pipewire::spa::buffer::ChunkFlags::CORRUPTED.bits();
        raw.chunk = &mut chunk;
        assert!(read(&raw).unwrap().is_none());
    }
}
