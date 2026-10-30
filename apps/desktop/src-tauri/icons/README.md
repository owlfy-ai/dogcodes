# dog.codes app icon

[`icon-master.png`](icon-master.png) is the approved square source: a full-bleed black square with a white negative-space sheepdog side profile. The upright ear, small rounded nose and smooth closed muzzle are preserved, and the dog is positioned for balanced inner margins. There is no baked-in white border or outer white tile. Platform masks supply the rounded app-icon shape. The website logo is unchanged.

Regenerate desktop PNG, ICNS, and ICO assets from the repository root with `pnpm icons`. The script uses the installed Tauri CLI and does not overwrite the separately maintained transparent website logo. For macOS, it generates a separate ICNS with an 824 × 824 rounded tile centered in a 1024 × 1024 transparent canvas. `tauri.conf.json` references the desktop outputs for development and bundles. The desktop UI imports the generated `128x128.png` for its in-app logo.

Tauri embeds a cached copy of the icon in the development executable. `build.rs` explicitly tracks the icon directory so changing these assets invalidates the build. Run `cargo build -p desktop --bin desktop` (or let `pnpm desktop:dev` rebuild) and restart the desktop app. Core exits and starts along with the app.

## Design provenance

The original side-profile artwork was simplified with the imagegen tool at the user's request. `icon-master.png` is the checked-in square source for Tauri icon generation. Exploratory variants remain local and are excluded from the source release under the [source evidence policy](../../../../docs/implementation/source-evidence.md).
