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

The actual two-peer test uses real local UDP/DTLS/SCTP, no external TURN service.
It does not validate public NAT traversal, TURN/TLS or mobile network handoff.
The Flutter client has not yet integrated this transport.
