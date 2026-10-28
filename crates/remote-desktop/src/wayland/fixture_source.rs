//! Synthetic producer compiled only into the opt-in Linux test executable.
use pipewire as pw;
use pw::spa::{
    param::{
        ParamType,
        format::{FormatProperties, MediaSubtype, MediaType},
        video::VideoFormat,
    },
    pod::{self, Pod, serialize::PodSerializer},
    sys,
    utils::{Direction, Fraction, Rectangle, SpaTypes},
};
use std::{collections::HashMap, io::Cursor, os::fd::AsRawFd, path::PathBuf, time::Duration};

fn serialize(object: pod::Object) -> Vec<u8> {
    PodSerializer::serialize(Cursor::new(Vec::new()), &pod::Value::Object(object))
        .unwrap()
        .0
        .into_inner()
}

#[test]
#[ignore = "private PipeWire synthetic producer; only the owned acceptance launcher starts it"]
fn serve_raw_source_fixture() {
    let root = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").expect("private runtime directory"));
    let marker = std::env::var("DOGCODES_WAYLAND_FIXTURE_MARKER").expect("owned fixture marker");
    assert!(!marker.is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join(".fixture-owned")).unwrap(),
        marker
    );
    assert_eq!(
        std::env::var("DOGCODES_OWNED_PORTAL").unwrap(),
        "portal-fixture-v1"
    );
    let mode = std::env::var("DOGCODES_OWNED_VIDEO_FORMAT").unwrap();
    let (matrix, y, u, v) = match mode.as_str() {
        "rgb" | "rgb-cursor" | "rgb-crop-rotate90" | "rgb-crop-flipped90" => {
            (sys::SPA_VIDEO_COLOR_MATRIX_RGB, 0, 0, 0)
        }
        "nv12-default" => (sys::SPA_VIDEO_COLOR_MATRIX_UNKNOWN, 81, 90, 240),
        "nv12-bt601" => (sys::SPA_VIDEO_COLOR_MATRIX_BT601, 81, 90, 240),
        "nv12-bt709" | "nv12-bt709-planes" => (sys::SPA_VIDEO_COLOR_MATRIX_BT709, 63, 102, 240),
        _ => panic!("unknown synthetic format"),
    };
    let rgb = mode.starts_with("rgb");
    let cursor = mode == "rgb-cursor";
    let transform = match mode.as_str() {
        "rgb-crop-rotate90" => 1,
        "rgb-crop-flipped90" => 5,
        _ => 0,
    };
    let height = if transform == 0 { 240usize } else { 400usize };
    let format = if rgb {
        VideoFormat::BGRx
    } else {
        VideoFormat::NV12
    };
    let separate = mode.ends_with("-planes");
    let pitch = if rgb {
        1280
    } else if separate {
        336
    } else {
        320
    };
    let prefix = if separate { 32 } else { 0 };
    let capacity = prefix
        + if separate || rgb {
            pitch * height
        } else {
            pitch * height * 3 / 2
        };
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).unwrap();
    let context = pw::context::ContextRc::new(&main_loop, None).unwrap();
    // This is the launcher's own server, never the default desktop connection.
    let socket = std::os::unix::net::UnixStream::connect(root.join("pipewire-0")).unwrap();
    let core = context.connect_fd_rc(socket.into(), None).unwrap();
    let stream = pw::stream::StreamRc::new(
        core,
        "owned NV12 source",
        pw::properties::properties! {
            "node.name" => "dogcodes-fixture", "media.class" => "Video/Source",
            "media.type" => "Video", "media.category" => "Capture", "media.role" => "Screen",
        },
    )
    .unwrap();
    // The source image includes both Y and UV even when each buffer plane
    // allocates only its own maximum pitch/height.
    let mut image = vec![
        y;
        if rgb {
            pitch * height
        } else {
            pitch * height * 3 / 2
        }
    ];
    if rgb {
        for y in 0..height {
            for x in 0..320usize {
                let color = if transform == 0 {
                    [0, 0, 255, 255]
                } else if !(40..280).contains(&x) || !(40..360).contains(&y) {
                    [255, 0, 255, 255] // Padding is deliberately visible if crop is lost.
                } else {
                    let left = x < 160;
                    let top = y < 200;
                    match (transform, left, top) {
                        (1, true, true) | (5, true, false) => [0, 255, 0, 255],
                        (1, false, true) | (5, false, false) => [255, 255, 255, 255],
                        (1, true, false) | (5, true, true) => [0, 0, 255, 255],
                        _ => [255, 0, 0, 255],
                    }
                };
                image[y * pitch + x * 4..y * pitch + x * 4 + 4].copy_from_slice(&color);
            }
        }
    } else {
        for pair in image[pitch * height..].chunks_exact_mut(2) {
            pair.copy_from_slice(&[u, v]);
        }
    }
    let _listener = stream
        .add_local_listener_with_user_data((image, HashMap::<usize, Vec<std::fs::File>>::new()))
        .add_buffer(move |_, state, buffer| {
            assert!(!buffer.is_null());
            // SAFETY: PipeWire grants this source exclusive initialization of
            // an ALLOC_BUFFERS buffer before publishing its plane descriptors.
            let raw = unsafe { &mut *(*buffer).buffer };
            assert_eq!(raw.n_datas as usize, if separate { 2 } else { 1 });
            if raw.n_metas > 0 {
                // SAFETY: ALLOC_BUFFERS grants this synthetic producer exclusive
                // initialization of all metadata before the buffer is shared.
                unsafe { initialize_fixture(raw, transform) }.unwrap();
            }
            if transform != 0 {
                assert!(raw.n_metas >= 3);
            }
            assert!(!raw.datas.is_null());
            // SAFETY: the live native buffer contains exactly n_datas slots.
            let datas = unsafe { std::slice::from_raw_parts_mut(raw.datas, raw.n_datas as usize) };
            let mut files = Vec::new();
            for data in datas {
                let file = tempfile::tempfile().unwrap();
                file.set_len(capacity as u64).unwrap();
                data.type_ = sys::SPA_DATA_MemFd;
                data.flags = pw::spa::buffer::DataFlags::READWRITE.bits();
                data.fd = i64::from(file.as_raw_fd());
                data.mapoffset = 0;
                data.maxsize = capacity as u32;
                data.data = std::ptr::null_mut();
                files.push(file);
            }
            assert!(state.1.insert(buffer as usize, files).is_none());
        })
        .remove_buffer(|_, state, buffer| {
            state.1.remove(&(buffer as usize));
        })
        .param_changed(move |stream, _, id, param| {
            if id != ParamType::Format.as_raw() || param.is_none() {
                return;
            }
            let bytes = serialize(pod::object!(
                SpaTypes::ObjectParamBuffers,
                ParamType::Buffers,
                pod::Property::new(sys::SPA_PARAM_BUFFERS_buffers, pod::Value::Int(4)),
                pod::Property::new(
                    sys::SPA_PARAM_BUFFERS_blocks,
                    pod::Value::Int(if separate { 2 } else { 1 })
                ),
                pod::Property::new(
                    sys::SPA_PARAM_BUFFERS_dataType,
                    pod::Value::Int(1 << sys::SPA_DATA_MemFd)
                ),
                pod::Property::new(
                    sys::SPA_PARAM_BUFFERS_size,
                    pod::Value::Int(capacity as i32)
                ),
                pod::Property::new(sys::SPA_PARAM_BUFFERS_align, pod::Value::Int(16)),
                pod::Property::new(sys::SPA_PARAM_BUFFERS_stride, pod::Value::Int(pitch as i32))
            ));
            let mut params = vec![bytes];
            params.extend(crate::wayland::metadata::parameters_for_cursor(cursor).unwrap());
            let mut pods = params
                .iter()
                .map(|bytes| Pod::from_bytes(bytes).unwrap())
                .collect::<Vec<_>>();
            stream.update_params(&mut pods).unwrap();
        })
        .process(move |stream, state| {
            let image = &state.0;
            let Some(mut buffer) = crate::wayland::loan::Loan::dequeue(stream) else {
                return;
            };
            let corrupt = root.join(".force-corrupt").exists();
            let corrupt_chunk = root.join(".force-corrupt-chunk").exists();
            let empty = root.join(".force-empty").exists();
            // SAFETY: this producer exclusively owns the loan and metadata.
            unsafe {
                set_corruption(buffer.raw().unwrap(), corrupt);
                if cursor {
                    set_cursor(
                        buffer.raw().unwrap(),
                        root.join(".cursor-moved").exists(),
                        root.join(".cursor-hidden").exists(),
                    );
                }
            };
            let observed = root.join(if corrupt {
                ".corrupt-written"
            } else if corrupt_chunk {
                ".corrupt-chunk-written"
            } else if empty {
                ".empty-written"
            } else {
                ".healthy-written"
            });
            if !observed.exists() {
                std::fs::write(observed, b"owned-source").unwrap();
            }
            let datas = buffer.datas_mut().unwrap();
            assert_eq!(datas.len(), if separate { 2 } else { 1 });
            for (index, data) in datas.iter_mut().enumerate() {
                let bytes = if separate {
                    if index == 0 {
                        &image[..pitch * height]
                    } else {
                        &image[pitch * height..]
                    }
                } else {
                    image.as_slice()
                };
                assert!(data.flags().contains(pw::spa::buffer::DataFlags::WRITABLE));
                {
                    let mut mapping =
                        crate::wayland::memory::Mapping::new(data.as_raw(), true).unwrap();
                    let memory = mapping.bytes_mut().unwrap();
                    memory.fill(77);
                    memory[prefix..prefix + bytes.len()].copy_from_slice(bytes);
                }
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = prefix as u32;
                *chunk.size_mut() = if empty { 0 } else { bytes.len() as u32 };
                *chunk.stride_mut() = pitch as i32;
                let flags = if corrupt_chunk && index == usize::from(separate) {
                    pw::spa::buffer::ChunkFlags::CORRUPTED.bits()
                } else {
                    0
                };
                // SAFETY: this producer exclusively owns the native loan and
                // its chunk, also borrowed by chunk_mut above. No borrow escapes.
                unsafe { (*data.as_raw().chunk).flags = flags };
            }
        })
        .register()
        .unwrap();
    let bytes = serialize(pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(FormatProperties::VideoFormat, Id, format),
        pod::property!(
            FormatProperties::VideoSize,
            Rectangle,
            Rectangle {
                width: 320,
                height: height as u32
            }
        ),
        pod::property!(
            FormatProperties::VideoFramerate,
            Fraction,
            Fraction { num: 30, denom: 1 }
        ),
        pod::property!(
            FormatProperties::VideoColorMatrix,
            pod::Value::Id(pw::spa::utils::Id(matrix))
        ),
        pod::property!(
            FormatProperties::VideoColorRange,
            pod::Value::Id(pw::spa::utils::Id(if mode == "nv12-default" {
                sys::SPA_VIDEO_COLOR_RANGE_UNKNOWN
            } else if rgb {
                sys::SPA_VIDEO_COLOR_RANGE_0_255
            } else {
                sys::SPA_VIDEO_COLOR_RANGE_16_235
            }))
        )
    ));
    let mut initial = vec![bytes];
    initial.extend(crate::wayland::metadata::parameters_for_cursor(cursor).unwrap());
    let mut pods = initial
        .iter()
        .map(|bytes| Pod::from_bytes(bytes).unwrap())
        .collect::<Vec<_>>();
    stream
        .connect(
            Direction::Output,
            None,
            pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::ALLOC_BUFFERS,
            &mut pods,
        )
        .unwrap();
    let ticking = stream.clone();
    let timer = main_loop.loop_().add_timer(move |_| {
        ticking.trigger_process().unwrap();
    });
    timer
        .update_timer(
            Some(Duration::from_millis(1)),
            Some(Duration::from_millis(33)),
        )
        .into_result()
        .unwrap();
    main_loop.run();
}

unsafe fn set_corruption(buffer: &sys::spa_buffer, corrupt: bool) {
    if buffer.n_metas == 0 {
        return;
    }
    assert!(buffer.n_metas <= 64 && !buffer.metas.is_null());
    // SAFETY: the dequeued native producer owns this bounded metadata array.
    let metas = unsafe { std::slice::from_raw_parts(buffer.metas, buffer.n_metas as usize) };
    for meta in metas.iter().filter(|m| m.type_ == sys::SPA_META_Header) {
        assert!(!meta.data.is_null() && meta.size as usize >= size_of::<sys::spa_meta_header>());
        // SAFETY: exclusive producer access to a fully present POD header.
        unsafe {
            let header = meta.data.cast::<sys::spa_meta_header>();
            let mut value = std::ptr::read_unaligned(header);
            if corrupt {
                value.flags |= sys::SPA_META_HEADER_FLAG_CORRUPTED;
            } else {
                value.flags &= !sys::SPA_META_HEADER_FLAG_CORRUPTED;
            }
            std::ptr::write_unaligned(header, value);
        }
    }
}

unsafe fn initialize_fixture(buffer: &mut sys::spa_buffer, transform: u32) -> crate::Result<()> {
    // SAFETY: only the owned synthetic producer calls this during its exclusive
    // initialization/process callback; PipeWire owns the checked descriptor array.
    if buffer.n_metas == 0 || buffer.n_metas > 64 || buffer.metas.is_null() {
        return Err("fixture metadata missing".into());
    }
    // SAFETY: the native producer owns the non-null bounded metadata array.
    let metas = unsafe { std::slice::from_raw_parts_mut(buffer.metas, buffer.n_metas as usize) };
    for meta in metas {
        if meta.type_ == sys::SPA_META_Header {
            assert!(
                !meta.data.is_null() && meta.size as usize >= size_of::<sys::spa_meta_header>()
            );
            // SAFETY: zero is valid for this SPA integer-only header. The owned
            // producer has exclusive access to its checked writable payload.
            unsafe {
                std::ptr::write_unaligned(
                    meta.data.cast::<sys::spa_meta_header>(),
                    std::mem::zeroed(),
                );
            }
        } else if meta.type_ == sys::SPA_META_VideoCrop {
            if (meta.size as usize) < size_of::<sys::spa_meta_region>() || meta.data.is_null() {
                return Err("fixture crop missing".into());
            }
            let mut value = sys::spa_meta_region {
                region: sys::spa_region {
                    position: sys::spa_point { x: 40, y: 40 },
                    size: sys::spa_rectangle {
                        width: 240,
                        height: 320,
                    },
                },
            };
            if transform == 0 {
                value.region.size.width = 0;
                value.region.size.height = 0;
            }
            // SAFETY: exclusive producer access and checked writable payload size.
            unsafe {
                std::ptr::write_unaligned(meta.data.cast::<sys::spa_meta_region>(), value);
            }
        } else if meta.type_ == sys::SPA_META_VideoTransform {
            if (meta.size as usize) < size_of::<sys::spa_meta_videotransform>()
                || meta.data.is_null()
            {
                return Err("fixture transform missing".into());
            }
            // SAFETY: exclusive producer access and checked writable payload size.
            unsafe {
                std::ptr::write_unaligned(
                    meta.data.cast::<sys::spa_meta_videotransform>(),
                    sys::spa_meta_videotransform { transform },
                );
            }
        }
    }
    Ok(())
}

// SAFETY: the synthetic producer exclusively owns all payloads in this buffer.
unsafe fn set_cursor(buffer: &sys::spa_buffer, moved: bool, hidden: bool) {
    assert!(buffer.n_metas <= 64 && !buffer.metas.is_null());
    // SAFETY: checked descriptor array belongs to this producer loan.
    let metas = unsafe { std::slice::from_raw_parts(buffer.metas, buffer.n_metas as usize) };
    let meta = metas
        .iter()
        .find(|m| m.type_ == sys::SPA_META_Cursor)
        .expect("negotiated cursor metadata");
    let header = size_of::<sys::spa_meta_cursor>();
    let bitmap = size_of::<sys::spa_meta_bitmap>();
    assert!(!meta.data.is_null() && meta.size as usize >= header + bitmap + 8 * 8 * 4);
    let cursor = sys::spa_meta_cursor {
        id: 1,
        flags: 0,
        position: sys::spa_point {
            x: if moved { 61 } else { 21 },
            y: 32,
        },
        hotspot: sys::spa_point { x: 1, y: 2 },
        bitmap_offset: if moved && !hidden { 0 } else { header as u32 },
    };
    let image = sys::spa_meta_bitmap {
        format: VideoFormat::BGRA.as_raw(),
        size: sys::spa_rectangle {
            width: 8,
            height: 8,
        },
        stride: 32,
        offset: if hidden { 0 } else { bitmap as u32 },
    };
    // SAFETY: full metadata extent was verified; this callback has exclusive write access.
    unsafe {
        let ptr = meta.data.cast::<u8>();
        std::ptr::write_unaligned(ptr.cast::<sys::spa_meta_cursor>(), cursor);
        std::ptr::write_unaligned(ptr.add(header).cast::<sys::spa_meta_bitmap>(), image);
        let bytes = std::slice::from_raw_parts_mut(ptr.add(header + bitmap), 8 * 8 * 4);
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[255, 0, 0, 255]);
        }
    }
}
