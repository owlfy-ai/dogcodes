# Endpoint transport

Reliable ordered `dogcodes-v1` WebRTC DataChannel with bounded messages/send buffers,
ICE gathering/connection deadlines, explicit/cancellation cleanup and selected-pair
diagnostics. `offer` / `answer` are transport only: callers must authenticate the
new channel with fresh Noise state and the existing paired identity pins before
dispatching any operations. Do not share a Noise session across transport paths.

Use independent business/desktop/preview/attachments channels. Signalling goes
over an already authenticated endpoint connection. Failed negotiation keeps the
working relay; a path change requires resolving existing operation receipts,
not repeating effects. `route()` reports direct / TURN / unknown without exposing
candidate addresses, SDP, credentials or certificates.

Ordinary two-peer tests use real local UDP/DTLS/SCTP and H.264 RTP, including
concurrent data-channel traffic and picture-loss feedback. The Flutter companion
integrates business and desktop connections through its RTC transport adapter.

`python3 scripts/turn_acceptance.py` starts a private coturn service with temporary
credentials and a local test CA. It forces both peers to use TURN for UDP, TCP,
TLS and a deliberately fragmented TCP stream; it checks encrypted ordered data,
RTP and concurrent control messages, plus certificate/name and unavailable-server
failures. The CA is passed only to test subprocesses, never installed globally.
The fixture tests remain opt-in; ignored cases do not establish acceptance.

Local tests do not establish public NAT traversal, production TURN/TLS operation,
mobile hardware decoding or real network handoff. See the
[current work list](../../docs/CURRENT_WORK.md) for the remaining cross-platform
and device acceptance.
