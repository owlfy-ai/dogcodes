# Native desktop boundary

Capture, display enumeration, input, manual text/image clipboard and idle-sleep
inhibition. Authentication, control leases and replay prevention live in the
daemon gateway. No audio or lock-screen unlock is implemented. OS lock-state
errors fail closed for desktop/input/clipboard, while Agent operations continue.

`video` uses ScreenCaptureKit → VideoToolbox on macOS, and a bounded
xcap screenshot worker → source-built OpenH264 on Windows and Linux X11.
Linux Wayland uses one connection-owned RemoteDesktop/ScreenCast portal and
its restricted PipeWire FD, shared by capture/JPEG/video/input. All
three can send H.264 through WebRTC (direct or TURN) and the authenticated WSS
relay. The software fallback uses Constrained Baseline 3.1, negotiated geometry
and live rate control, suppresses unchanged frames, and does not download a
codec at runtime. Native accelerated Windows/Linux video remains pending.
If video is unavailable, the existing bounded JPEG pull remains available
(up to 25fps, one frame in flight, unchanged frames answered without bytes).
See `docs/implementation/remote-desktop-video-2026-10-21.md` for limits,
measurements and remaining work; `examples/frame_bench.rs` and
`examples/video_bench.rs --synthetic` measure capture/encode cost locally.
Wayland portal lifecycle, restore tokens and the continuous packed-RGB path are
implemented; compositor permissions, Unicode/IME, DPI, lock-screen and restore
behavior still require real-environment validation. See
`docs/implementation/wayland-remote-desktop-2026-10-22.md` for the boundary and
isolated D-Bus + real PipeWire acceptance fixture. Native hardware encoding and
DMA-BUF-only sources remain pending. Do not treat compilation or exposed
methods as real-device acceptance.

Linux build prerequisites (Debian/Ubuntu, in addition to Tauri dependencies):

```sh
sudo apt-get install pkg-config libclang-dev libxcb1-dev libxrandr-dev libdbus-1-dev libpipewire-0.3-dev libwayland-dev libegl-dev libgbm-dev libxkbcommon-dev nasm
```

Desktop/release workflows install these dependencies. Native macOS tests,
Windows GNU cross-compilation, and an isolated Linux Xvfb capture → software
encode → decode test run locally. The desktop workflow also runs that Xvfb
check (`-noreset` keeps its owned desktop alive between capture connections).
MSVC packaging, real Windows/Linux displays, Wayland, permission/input/DPI,
and lock-screen scenarios still require the corresponding environments.
Tests do not capture the user's screen or inject real input.
