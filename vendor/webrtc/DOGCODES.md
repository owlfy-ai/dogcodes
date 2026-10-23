# DogCodes TURN stream extension

Library-only snapshot of crates.io `webrtc 0.21.1` (webrtc-rs, MIT OR Apache-2.0).
Original licenses are retained. Only the async adapter is vendored: the `rtc`
protocol core and all other dependencies remain registry packages pinned by the
workspace lockfile. RustDesk code and protocols are not copied.

`[patch.crates-io]` selects this snapshot; `dogcodes-remote-transport` enables the
additive `turn-stream` feature. This feature uses the project's Tokio runtime.
Without it the UDP/runtime APIs remain available.

Changes to the upstream snapshot:

- `Cargo.toml`: library-only manifest (examples/integration fixtures are omitted),
  `turn-stream` feature, Tokio I/O and Rustls/native/public trust dependencies.
- `src/peer_connection/transport/turn_stream.rs`: TCP and TLS dialing, parallel
  address attempts, strict server certificate/name verification, RFC 8656
  STUN/ChannelData stream framing, bounded queues/write deadlines and cleanup.
- `transport/turn_relayer.rs`: TCP/TLS clients use the same sans-I/O TURN client
  for authentication, allocation/refresh, permissions/channel bindings and release.
  Independent server dials run concurrently; streams do not masquerade as UDP.
- `transport/mod.rs`: conditionally include the extension.
- `peer_connection/driver.rs`: route TURN TCP separately from ICE TCP (which uses
  RFC 4571), feed reads/errors into the TURN client, flush authenticated release
  messages before retiring streams, and keep live TURN streams when UDP sockets
  fail. Absent optional socket sets now park rather than busy-loop `Ready(None)`.

TLS trusts public roots and platform roots, including explicitly configured
`SSL_CERT_FILE` / `SSL_CERT_DIR` as supported by rustls-native-certs. There is no
insecure certificate/name bypass. The local test CA is passed only to isolated
test subprocesses and is never installed in OS trust. `turns:` is TLS over TCP;
TURN/DTLS over UDP and TURN-over-WebSocket are not implemented.

Review/verification: `python3 scripts/turn_acceptance.py` at the workspace root
forces relay-only candidates on both endpoints against an isolated coturn. It
checks UDP/TCP/TLS, deliberately fragmented TCP byte streams, fresh pinned Noise
and ordered encrypted payloads, untrusted/name-mismatched certificates and an
unavailable endpoint. The normal desktop/phone gateway tests remain unchanged.

When upgrading, compare the files above with the new upstream implementation,
remove this patch if equivalent support is present, and rerun forced-relay,
certificate-failure, direct-connect and phone fallback tests. Do not silently
replace this snapshot with the registry version, whose TURN relayer skips TLS/TCP.
