//! Real x11rb requests on a private socket; no DISPLAY, Xauthority or Enigo.
use super::*;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use x11rb::{
    protocol::{randr, xproto},
    rust_connection::DefaultStream,
    x11_utils::Serialize,
};
fn wire<T: Serialize>(value: T) -> Vec<u8>
where
    T::Bytes: AsRef<[u8]>,
{
    let mut bytes = value.serialize().as_ref().to_vec();
    bytes.resize(bytes.len().next_multiple_of(4).max(32), 0);
    let length = u32::try_from((bytes.len() - 32) / 4).unwrap();
    bytes[4..8].copy_from_slice(&length.to_ne_bytes());
    bytes
}
#[test]
fn real_randr_queries_refresh_root_pixels_and_warp_targets_the_selected_root() {
    let (client, mut server) = UnixStream::pair().unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let bounds = Arc::new(Mutex::new(Bounds {
        x: 3840,
        y: 0,
        width: 3840,
        height: 2160,
    }));
    let moves = Arc::new(Mutex::new(vec![]));
    let (native_bounds, observed) = (bounds.clone(), moves.clone());
    let worker = thread::spawn(move || {
        let mut connect = [0; 12];
        server.read_exact(&mut connect).unwrap();
        assert_eq!(
            u16::from_ne_bytes(connect[6..8].try_into().unwrap()),
            0,
            "must not read or send user authentication"
        );
        let mut setup = xproto::Setup {
            status: 1,
            protocol_major_version: 11,
            resource_id_base: 0x100000,
            resource_id_mask: 0xfffff,
            maximum_request_length: 65535,
            min_keycode: 8,
            max_keycode: 255,
            roots: vec![xproto::Screen {
                root: 0x123,
                width_in_pixels: 8192,
                height_in_pixels: 8192,
                root_depth: 24,
                ..Default::default()
            }],
            ..Default::default()
        };
        setup.length = u16::try_from((setup.serialize().len() - 8) / 4).unwrap();
        server.write_all(&setup.serialize()).unwrap();
        let mut seq = 0u16;
        loop {
            let mut head = [0; 4];
            if server.read_exact(&mut head).is_err() {
                break;
            }
            seq = seq.wrapping_add(1);
            let size = usize::from(u16::from_ne_bytes(head[2..4].try_into().unwrap())) * 4;
            assert!((4..=4096).contains(&size));
            let mut request = vec![0; size];
            request[..4].copy_from_slice(&head);
            server.read_exact(&mut request[4..]).unwrap();
            match head[0] {
                xproto::QUERY_EXTENSION_REQUEST => {
                    let len = usize::from(u16::from_ne_bytes(request[4..6].try_into().unwrap()));
                    let present = &request[8..8 + len] == b"RANDR";
                    server
                        .write_all(&wire(xproto::QueryExtensionReply {
                            sequence: seq,
                            present,
                            major_opcode: 128,
                            ..Default::default()
                        }))
                        .unwrap();
                }
                128 if head[1] == randr::GET_OUTPUT_INFO_REQUEST => {
                    assert_eq!(u32::from_ne_bytes(request[4..8].try_into().unwrap()), 7);
                    server
                        .write_all(&wire(randr::GetOutputInfoReply {
                            status: randr::SetConfig::SUCCESS,
                            sequence: seq,
                            crtc: 3,
                            connection: randr::Connection::CONNECTED,
                            name: b"fixture".to_vec(),
                            ..Default::default()
                        }))
                        .unwrap();
                }
                128 if head[1] == randr::GET_CRTC_INFO_REQUEST => {
                    let b = *native_bounds.lock().unwrap();
                    server
                        .write_all(&wire(randr::GetCrtcInfoReply {
                            status: randr::SetConfig::SUCCESS,
                            sequence: seq,
                            x: i16::try_from(b.x).unwrap(),
                            y: i16::try_from(b.y).unwrap(),
                            width: u16::try_from(b.width).unwrap(),
                            height: u16::try_from(b.height).unwrap(),
                            outputs: if b.width > 0 { vec![7] } else { vec![] },
                            ..Default::default()
                        }))
                        .unwrap();
                }
                xproto::WARP_POINTER_REQUEST => {
                    observed.lock().unwrap().push((
                        u32::from_ne_bytes(request[8..12].try_into().unwrap()),
                        i16::from_ne_bytes(request[20..22].try_into().unwrap()),
                        i16::from_ne_bytes(request[22..24].try_into().unwrap()),
                    ));
                }
                xproto::GET_INPUT_FOCUS_REQUEST => {
                    server
                        .write_all(&wire(xproto::GetInputFocusReply {
                            sequence: seq,
                            ..Default::default()
                        }))
                        .unwrap();
                }
                _ => panic!("unexpected isolated X11 request: {} {}", head[0], head[1]),
            }
        }
    });
    let (stream, _) = DefaultStream::from_unix_stream(client).unwrap();
    let connection = RustConnection::connect_to_stream(stream, 0).unwrap();
    let resolver = Resolver(OnceLock::from(Ok(Native {
        connection,
        root: 0x123,
    })));
    let stale = Display {
        id: 7,
        name: "fixture".into(),
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 2.0,
        primary: true,
    };
    let current = resolver.read(&stale).unwrap();
    assert_eq!(
        (current.x, current.width, current.height),
        (3840, 3840, 2160)
    );
    let (x, y) = crate::to_global(&current, u16::MAX, u16::MAX);
    resolver.move_root(x, y).unwrap();
    *bounds.lock().unwrap() = Bounds {
        x: 0,
        y: 100,
        width: 1080,
        height: 1920,
    };
    let changed = resolver.read(&stale).unwrap();
    assert_eq!(
        (changed.x, changed.y, changed.width, changed.height),
        (0, 100, 1080, 1920)
    );
    let (x, y) = crate::to_global(&changed, u16::MAX, u16::MAX);
    resolver.move_root(x, y).unwrap();
    assert_eq!(
        *moves.lock().unwrap(),
        vec![(0x123, 7679, 2159), (0x123, 1079, 2019)]
    );
    assert!(resolver.move_root(-1, 0).is_err());
    assert!(resolver.move_root(40_000, 0).is_err());
    assert_eq!(moves.lock().unwrap().len(), 2);
    *bounds.lock().unwrap() = Bounds {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };
    assert!(resolver.read(&stale).is_err());
    assert_eq!(
        moves.lock().unwrap().len(),
        2,
        "disconnected screen must not cause a stale move"
    );
    drop(resolver);
    worker.join().unwrap();
}
