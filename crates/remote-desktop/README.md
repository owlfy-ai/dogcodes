# Native desktop boundary

The macOS runtime requires macOS 12.3 or later: ScreenCaptureKit is a required
linked framework. The desktop bundle declares the same minimum system version.

Native input now resolves current display bounds per positional batch: macOS logical points, Windows physical desktop coordinates under a scoped thread DPI context, and X11 RandR root pixels with an explicit root-window warp. Capture metadata is not a fallback for removed displays. Keyboard/releases remain available without geometry; the gateway rechecks the original owner and deadline before later actions. Wayland keeps its original portal/EIS mapping. Code and isolated tests do not replace real monitor/DPI/hotplug validation; see ../../docs/implementation/input-geometry-2026-10-26.md.

Read-only `cursor::Reader` supplies native Windows/X11 pointer images and hotspots without constructing an input controller. Its per-lease PNG cache is acknowledged through existing desktop replies; Portal/embedded sources stay on their original capture. When Portal offers cursor metadata instead of embedded pixels, its bounded bitmap/position is composited onto one clean background before JPEG/H.264, including cursor-only updates. macOS JPEG lazily uses an owned ScreenCaptureKit BGRA stream with the system cursor included; if that backend fails, the original screenshot and position-only overlay remain available. See ../../docs/implementation/native-cursor-2026-10-27.md for protocol, bounds and hardware validation limits.

DXGI pointer metadata stays unknown until a valid mouse update, survives unchanged pixels, and resets with its capture source. Its overlay mode travels with each input/encoded frame; only current, transport-accepted output updates the gateway description. This is not proof of which RTP frame the client has decoded.

Capture, display enumeration, input, manual text/image clipboard and idle-sleep
inhibition. Authentication, control leases and replay prevention live in the
daemon gateway. No audio or lock-screen unlock is implemented. OS lock-state
errors fail closed for desktop/input/clipboard, while Agent operations continue.

`video` uses ScreenCaptureKit → VideoToolbox on macOS, and a bounded
Desktop Duplication/bounded GDI → hardware Media Foundation → source-built
OpenH264 fallback on Windows. Linux X11 and Wayland can select NVENC/VA-API
through GStreamer, with the same source-built OpenH264 fallback.
Windows DXGI can normalize FP16 scRGB to the existing SDR video and JPEG input
using the original display's SDR white level. JPEG reuses the same pull/recovery
controller and bounded native worker pool, retires while idle, and retains its
original desktop lease. Synthetic conversion/codec/JPEG tests pass; actual HDR
displays, video/image switching and GDI fallback color quality still require acceptance.
Linux Wayland uses one connection-owned RemoteDesktop/ScreenCast portal and
its restricted PipeWire FD, shared by capture/JPEG/video/input. All
three can send H.264 through WebRTC (direct or TURN) and the authenticated WSS
relay. The software fallback uses Constrained Baseline 3.1, negotiated geometry
and live rate control, suppresses unchanged frames, and does not download a
codec at runtime. Windows hardware encoding has a bounded COM worker, actual
synthetic warmup/decode validation and a software IDR takeover; real Windows
GPU/driver testing remains pending. Linux uses a bounded codec worker and
actual warmup decode before selection; real Linux GPU/driver acceptance is
still pending. See [Linux implementation and limits](../../docs/implementation/linux-hardware-video-2026-10-26.md) and
`docs/implementation/windows-native-video-2026-10-25.md` for evidence and limits.
If video is unavailable, the existing bounded JPEG pull remains available
(up to 25fps, one frame in flight, unchanged frames answered without bytes).
See `docs/implementation/remote-desktop-video-2026-10-21.md` for limits,
measurements and remaining work; `examples/frame_bench.rs` and
`examples/video_bench.rs --synthetic` measure capture/encode cost locally.
Wayland portal lifecycle, restore tokens and continuous packed-RGB/NV12 paths are
implemented; compositor permissions, Unicode/IME, DPI, lock-screen and restore
behavior still require real-environment validation. See
`docs/implementation/wayland-remote-desktop-2026-10-22.md` for the boundary and
isolated D-Bus + real PipeWire acceptance fixture. The NV12 path supports one or
two mapped planes, independent offsets/strides, and BT.601/709 SDR metadata;
unspecified colorimetry defaults to BT.601 limited. Five source modes run in the
desktop workflow; see `docs/implementation/wayland-nv12-sources-2026-10-25.md`.
DMA-BUF formats/modifiers now have an optional EGL external-image import and
RGBA readback path, with bounded renegotiation back to GPU/SHM alternatives.
It is not a zero-copy encoder path. The Linux hardware backend now accepts
these normalized CPU frames; real GPU, DMA-BUF, multi-GPU and compositor
testing remain pending; see
`docs/implementation/wayland-dmabuf-egl-2026-10-25.md`.
Do not treat compilation or exposed methods as real-device acceptance.

Live video and JPEG revalidate the desktop state after encoding. A retired or
locked stream cannot resume from late codec output. Linux reads fresh login1
properties through one bounded D-Bus worker, including this user's graphical
Display session for a background Core; errors fail closed. Real compositor
lock/user-switch behavior remains to be tested. RustDesk design comparisons,
validation and remaining work are recorded in
`docs/implementation/rustdesk-video-reference-2026-10-25.md`.

Wayland now reads bounded buffer metadata, crops in storage coordinates and
normalizes all eight SPA orientations before JPEG/H.264. Both paths revalidate
the original portal source after encoding. Seven owned source modes check
orientation and decoded color; see
`docs/implementation/wayland-presentation-grants-2026-10-25.md`.
Automatic continuation across changes in the final visible geometry, explicit
sync and real compositor/GPU/device testing remain pending.

Linux build prerequisites (Debian/Ubuntu, in addition to Tauri dependencies):

```sh
sudo apt-get install python3 pkg-config libclang-dev libxcb1-dev libxrandr-dev libdbus-1-dev libpipewire-0.3-dev libpipewire-0.3-modules libspa-0.2-modules libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libwayland-dev libegl-dev libgbm-dev libxkbcommon-dev nasm file patchelf
spa_sdk_env="$(python3 scripts/prepare_linux_spa_sdk.py --print-env)" && eval "$spa_sdk_env"
```

Linux runtime requires GStreamer core/base libraries >= 1.20 (declared in the
DEB dependencies). Hardware plugins and matching vendor drivers are optional;
missing/failed `nvh264enc`, `vah264enc`, `vaapih264enc`, conversion or H.264 parser
elements select the source-built software fallback. Debian/Ubuntu installations
may provide the optional plugins through `gstreamer1.0-plugins-base`,
`gstreamer1.0-plugins-bad` and the older `gstreamer1.0-vaapi`; nothing is installed
or downloaded by a remote connection. The AppImage packaging command also
includes the matching SPA support and native/client-node modules with a minimal
client configuration; its launcher sets their paths before executing the app.
This client connects only through the original portal FD and does not start a
PipeWire server. Actual GPU plugins and distribution acceptance remain pending.

Desktop/release workflows install these dependencies. Native macOS tests,
Windows GNU cross-compilation, and an isolated Linux Xvfb capture → software
encode → decode test run locally. The desktop workflow also runs that Xvfb
check (`-noreset` keeps its owned desktop alive between capture connections).
MSVC packaging, real Windows/Linux displays, Wayland, permission/input/DPI,
and lock-screen scenarios still require the corresponding environments.
Tests do not capture the user's screen or inject real input.

Explicitly corrupted Wayland headers/chunks have one bounded recovery deadline;
new valid pixels clear it, while static or empty updates are normal. The desktop
codec migration acceptance now runs with a private login1 service via
`python3 scripts/desktop_codec_acceptance.py`. See
`docs/implementation/video-corruption-codec-acceptance-2026-10-25.md` for evidence
and remaining device/geometry recovery work.

Static refinement now uses real output feedback, bounded attempts/outputs/time
and a separate no-output/IDR watchdog. Tiny outputs can stop redundant repeats;
static stillness alone is not an error. Synthetic encode/decode measurements and
limits are in `docs/implementation/video-static-refresh-2026-10-25.md`. Hardware
quality calibration and real network/decoder feedback remain open.

Live streams can apply downstream admission before converting/encoding input,
retain one latest unsubmitted picture and fence late pre-pause output by the
original codec input time. macOS live submission also waits for callback credit.
See `docs/implementation/video-output-admission-2026-10-25.md`.

The macOS codec now creates, uses and destroys VideoToolbox state on a bounded
worker, with separate five-second initialization and two-second operation
deadlines. Its callback delivers results to a single-slot channel; it never
owns the real publishing sink. Timed-out native state retains its worker slot
until it actually exits, so reconnects cannot create unlimited hung threads.
See `docs/implementation/macos-codec-isolation-2026-10-25.md`. This does not
terminate stuck drivers; process-level recovery, real decoder/network behavior
and device recovery remain open.

ScreenCaptureKit now has a separate bounded capture pool. Its native objects
stay on the worker; startup and configuration have ten/two-second facade
deadlines. Parent lease cancellation prevents late initialization from starting
an abandoned recording, and delegate stop immediately fences the original
stream's encoded output. Native state retains its slot until actual exit;
unknown stop completion does not count as successful cleanup. See
`docs/implementation/macos-capture-isolation-2026-10-25.md` for isolated tests,
the corrected build-cache evidence, and remaining real OS/process validation.

Linux GStreamer input now wraps the existing immutable `Arc<RgbaImage>`
in read-only memory instead of cloning its full RGBA vector on every input.
The original pixels remain alive through native references; a requested write
map uses copy-on-write. This removes one CPU copy, not GPU uploads or color
conversion. Memory ownership/COW and synthetic decoding pass on GStreamer
1.20 and 1.22; actual GPU/driver performance remains to be measured. See
`docs/implementation/linux-hardware-video-2026-10-26.md`.

The same capture worker supervises the macOS JPEG source, including original
lease liveness during asynchronous preparation, display changes, and physical
stop acknowledgement. It caches one BGRA buffer, converts only on image pulls,
and stops after five seconds without a pull. An idle restart has separate
frame/geometry state so late stop callbacks cannot clear its replacement.
Backend failure falls back to xcap without repeatedly starting SCK streams;
the existing `cursorEmbedded` reply tells the phone which overlay to use.
Synthetic buffers verify padded rows, color order and publication guards;
actual system cursor/compositor behavior still needs device validation.

Native pool acquisition now briefly waits for real retirement notifications,
within the original absolute initialization deadline and at most two seconds.
The two-slot limits remain; a stuck context is never counted as already freed.
Both pool stress tests and repeated synthetic VideoToolbox rebuild/decode are
recorded in `docs/implementation/native-retirement-grace-2026-10-26.md`.

Source geometry changes on the software capture path fence old encoding/output and rebuild under the same source grant with a new video generation. The phone accepts that generation even when the user width sequence is unchanged. On Wayland, changed pixels do not establish logical pointer bounds: EIS uses the selected stream's mapping_id and authoritative device regions, with queued input fenced across device/keyboard generations. Older portals without mapping IDs keep Notify and require a fresh grant after pixel geometry changes. The session-owned EIS backend handles pointer, keyboard and text together; it never mixes Notify after obtaining the EIS FD. Backends without ei_text can plan mixed shortcuts/text using their keymap and EI feedback boundaries; unmappable Unicode can use the original session’s authorized Clipboard portal, using optional read-only AT-SPI focus roles to choose Ctrl+Shift+V for terminals and one Ctrl+V otherwise. It never retries another shortcut after an uncertain effect or adds a manual selector. This fallback never reads/backups the original clipboard or restores it afterward; later external ownership is preserved. Unexposed terminal roles, custom shortcuts, non-Latin V mapping and old portals without text/clipboard capabilities still require compatibility work and real-device verification. See [paste implementation and remaining scope](../../docs/implementation/wayland-clipboard-paste-2026-10-26.md). See [mixed input evidence](../../docs/implementation/wayland-eis-mixed-input-2026-10-26.md). See [EIS implementation and remaining scope](../../docs/implementation/wayland-eis-input-2026-10-26.md), [dynamic video geometry](../../docs/implementation/video-dynamic-geometry-2026-10-26.md), and [macOS native geometry](../../docs/implementation/macos-display-reconfiguration-2026-10-26.md). Actual compositor/display verification remains open.

macOS now observes display begin/commit notifications and checks geometry once per second on its bounded capture worker, including background Cores without Cocoa event delivery. Begin suspends old encoding/publication; commit requests the existing generation rebuild. It never queries geometry or unregisters inside the OS callback. See [implementation and verification limits](../../docs/implementation/macos-display-reconfiguration-2026-10-26.md); real mode switches and GPU recovery remain to be validated.

Windows video now prefers D3D11 Desktop Duplication, with bounded actor-owned DXGI and GDI fallback, independent of the codec pool. Idle surfaces skip readback; access loss gets limited recovery and replacement-first-frame verification. Parent lease cancellation and local stream retirement are distinct, and a recovered picture is installed before publication resumes. This remains CPU RGBA normalization rather than GPU-to-encoder zero copy. See [implementation, tests and Windows runtime limits](../../docs/implementation/windows-desktop-duplication-2026-10-26.md).
