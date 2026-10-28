//! Bounded metadata copies; pointers never outlive the original buffer loan.
use super::geometry::Presentation;
use crate::Result;
use pipewire::spa::{
    param::ParamType,
    pod::{self, serialize::PodSerializer},
    sys,
    utils::SpaTypes,
};
use std::io::Cursor;

// All call sites use SPA integer-only POD structs, including potentially
// unaligned metadata. A known tag must be unique, non-null and fully present.
unsafe fn one<T: Copy>(metas: &[sys::spa_meta], kind: u32) -> Result<Option<T>> {
    let mut values = metas.iter().filter(|meta| meta.type_ == kind);
    let Some(meta) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() || meta.data.is_null() || (meta.size as usize) < size_of::<T>() {
        return Err("Wayland 画面元数据不完整或重复".into());
    }
    // SAFETY: caller restricts T to SPA PODs valid for all bit patterns; the
    // loan owns this payload, and its declared extent was checked above.
    Ok(Some(unsafe {
        std::ptr::read_unaligned(meta.data.cast::<T>())
    }))
}
/// # Safety
/// Descriptor pointers must belong to the still-dequeued native buffer.
pub(super) unsafe fn read(buffer: &sys::spa_buffer) -> Result<Option<Presentation>> {
    if buffer.n_metas > 64 || (buffer.n_metas > 0 && buffer.metas.is_null()) {
        return Err("Wayland 画面元数据描述符无效".into());
    }
    let metas = if buffer.n_metas == 0 {
        &[]
    } else {
        // SAFETY: caller owns the native array; non-null and count were checked.
        unsafe { std::slice::from_raw_parts(buffer.metas, buffer.n_metas as usize) }
    };
    // SyncTimeline (type 9, introduced after our baseline SDK) requires explicit
    // acquire/release handling and must not be mistaken for implicit sync.
    if metas.iter().any(|m| m.type_ == 9) {
        return Err("Wayland 显式同步来源暂不支持".into());
    }
    // SAFETY: the tag matches its integer-only SPA header and the payload is
    // checked by one. Corrupt frames are dropped before reading other payloads.
    let header = unsafe { one::<sys::spa_meta_header>(metas, sys::SPA_META_Header)? };
    if header.is_some_and(|m| m.flags & sys::SPA_META_HEADER_FLAG_CORRUPTED != 0) {
        return Ok(None);
    }
    // SAFETY: each requested tag corresponds exactly to its integer-only SPA
    // POD; all payload extents are checked by one before a bounded copy.
    let (crop, transform) = unsafe {
        (
            one::<sys::spa_meta_region>(metas, sys::SPA_META_VideoCrop)?,
            one::<sys::spa_meta_videotransform>(metas, sys::SPA_META_VideoTransform)?,
        )
    };
    let crop = crop.and_then(|m| {
        let r = m.region;
        (r.size.width != 0 && r.size.height != 0).then_some((
            r.position.x,
            r.position.y,
            r.size.width,
            r.size.height,
        ))
    });
    let transform = transform.map_or(0, |m| m.transform);
    if transform > 7 {
        return Err("Wayland 画面方向元数据无效".into());
    }
    Ok(Some(Presentation { crop, transform }))
}
pub(super) fn parameters_for_cursor(cursor: bool) -> Result<Vec<Vec<u8>>> {
    let mut kinds = vec![
        (sys::SPA_META_Header, size_of::<sys::spa_meta_header>()),
        (sys::SPA_META_VideoCrop, size_of::<sys::spa_meta_region>()),
        (
            sys::SPA_META_VideoTransform,
            size_of::<sys::spa_meta_videotransform>(),
        ),
    ];
    if cursor {
        kinds.push((sys::SPA_META_Cursor, super::cursor::MAX_BYTES));
    }
    kinds
        .into_iter()
        .map(|(kind, size)| {
            let object = pod::object!(
                SpaTypes::ObjectParamMeta,
                ParamType::Meta,
                pod::Property::new(
                    sys::SPA_PARAM_META_type,
                    pod::Value::Id(pipewire::spa::utils::Id(kind))
                ),
                pod::Property::new(sys::SPA_PARAM_META_size, pod::Value::Int(size as i32))
            );
            PodSerializer::serialize(Cursor::new(Vec::new()), &pod::Value::Object(object))
                .map(|v| v.0.into_inner())
                .map_err(|_| "Wayland 元数据协商失败".into())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn meta<T>(kind: u32, value: &mut T) -> sys::spa_meta {
        sys::spa_meta {
            type_: kind,
            size: size_of::<T>() as u32,
            data: std::ptr::from_mut(value).cast(),
        }
    }
    fn parse(metas: &mut [sys::spa_meta]) -> Result<Option<Presentation>> {
        let buffer = sys::spa_buffer {
            n_metas: metas.len() as u32,
            metas: metas.as_mut_ptr(),
            n_datas: 0,
            datas: std::ptr::null_mut(),
        };
        // SAFETY: this test owns the exact array and live POD payloads through
        // the synchronous read; deliberately invalid sizes/nulls are rejected.
        unsafe { read(&buffer) }
    }
    #[test]
    fn known_metadata_is_copied_without_retaining_native_pointers() {
        let mut crop = sys::spa_meta_region {
            region: sys::spa_region {
                position: sys::spa_point { x: 2, y: 3 },
                size: sys::spa_rectangle {
                    width: 5,
                    height: 7,
                },
            },
        };
        let mut transform = sys::spa_meta_videotransform { transform: 5 };
        let presentation = parse(&mut [
            meta(sys::SPA_META_VideoCrop, &mut crop),
            meta(sys::SPA_META_VideoTransform, &mut transform),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(presentation.crop, Some((2, 3, 5, 7)));
        assert_eq!(presentation.transform, 5);
        assert_eq!(parse(&mut []).unwrap(), Some(Presentation::default()));
        for (width, height) in [(0, 0), (0, 7), (5, 0)] {
            crop.region.size = sys::spa_rectangle { width, height };
            assert_eq!(
                parse(&mut [meta(sys::SPA_META_VideoCrop, &mut crop)])
                    .unwrap()
                    .unwrap()
                    .crop,
                None
            );
        }
    }
    #[test]
    fn truncated_null_duplicate_and_unknown_orientation_do_not_hide_as_absent_metadata() {
        let mut transform = sys::spa_meta_videotransform { transform: 0 };
        let original = meta(sys::SPA_META_VideoTransform, &mut transform);
        let mut short = original;
        short.size = 1;
        short.data = std::ptr::dangling_mut::<u8>().cast();
        assert!(parse(&mut [short]).is_err());
        let mut null = original;
        null.data = std::ptr::null_mut();
        assert!(parse(&mut [null]).is_err());
        assert!(parse(&mut [original, original]).is_err());
        transform.transform = 8;
        assert!(parse(&mut [meta(sys::SPA_META_VideoTransform, &mut transform)]).is_err());
    }
    #[test]
    fn corrupted_header_drops_the_frame_before_touching_other_payloads() {
        // SAFETY: spa_meta_header is an integer-only POD with valid zero fields.
        let mut header: sys::spa_meta_header = unsafe { std::mem::zeroed() };
        header.flags = sys::SPA_META_HEADER_FLAG_CORRUPTED;
        let bad = sys::spa_meta {
            type_: sys::SPA_META_VideoCrop,
            size: 0,
            data: std::ptr::null_mut(),
        };
        assert!(
            parse(&mut [meta(sys::SPA_META_Header, &mut header), bad])
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn unaligned_payload_is_safe_and_explicit_sync_is_not_assumed_implicit() {
        let mut bytes = [0u8; 5];
        bytes[1..].copy_from_slice(&3u32.to_ne_bytes());
        let raw = sys::spa_meta {
            type_: sys::SPA_META_VideoTransform,
            size: 4,
            data: bytes.as_mut_ptr().wrapping_add(1).cast(),
        };
        assert_eq!(parse(&mut [raw]).unwrap().unwrap().transform, 3);
        assert!(
            parse(&mut [sys::spa_meta {
                type_: 9,
                size: 0,
                data: std::ptr::null_mut()
            }])
            .is_err()
        );
        let raw = sys::spa_buffer {
            n_metas: 65,
            metas: std::ptr::dangling_mut(),
            n_datas: 0,
            datas: std::ptr::null_mut(),
        };
        // SAFETY: read rejects the excessive count without dereferencing this pointer.
        assert!(unsafe { read(&raw) }.is_err());
    }
}
