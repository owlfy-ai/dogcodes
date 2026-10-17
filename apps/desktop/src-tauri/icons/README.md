# dogcodes app icon

[`icon-master.png`](icon-master.png) is a 1024 × 1024 square source containing the selected white dog profile with a lime-green ear on a black background. It is also the source for the website logo.

Regenerate desktop PNG, ICNS, and ICO assets and the website logo from the repository root with `pnpm icons`. The script uses the installed Tauri CLI and copies its 512 × 512 PNG to `website/assets/logo.png`, so both products use the same artwork. For macOS, it generates a separate ICNS with an 824 × 824 rounded black tile centered in a 1024 × 1024 transparent canvas. The Dock displays these ICNS pixels directly, so the transparent padding and rounded corners prevent a full-size black square. `tauri.conf.json` references the desktop outputs for development and bundles. The desktop UI also imports the generated `128x128.png` for its in-app logo.

Tauri embeds a cached copy of the icon in the development executable. `build.rs` explicitly tracks the icon directory so changing these assets invalidates the build. A restart of an old executable alone cannot update the Dock icon. Run `cargo build -p desktop --bin desktop` (or let `pnpm desktop:dev` rebuild) and restart the desktop UI. The Core service does not need to restart.

## Design provenance

The user selected the source image. `icon-master.png` is the checked-in square source for Tauri icon generation. Exploratory variants remain local and are excluded from the source release under the [source evidence policy](../../../../docs/implementation/source-evidence.md).
