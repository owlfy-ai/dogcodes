# dogcodes app icon

[`icon-master.png`](icon-master.png) is the checked-in image explicitly selected by the user. This version keeps the lime-green `<` and `>` ears, narrow white forehead blaze, white muzzle with its original pointed chin, and charcoal background. Its oval eyes have dark pupils and a small white highlight. The ears are slightly smaller relative to the face, and the complete foreground is enlarged to approximately 90% of the background tile width. Apply this source without redesigning or changing its proportions.

The visible tile occupies approximately 80% of the square canvas, leaving about 10% transparent padding per side so the macOS Dock icon does not appear oversized. Do not trim the transparent canvas.

Regenerate desktop PNG, ICNS, and ICO assets from the repository root with `pnpm icons`. The script uses the installed Tauri CLI and preserves transparency. `tauri.conf.json` references the outputs for development and future bundles; packaging remains controlled by the existing bundle settings.

Tauri embeds a cached copy of the icon in the development executable. `build.rs` explicitly tracks the icon directory so changing these assets invalidates the build. A restart of an old executable alone cannot update the Dock icon. Run `cargo build -p desktop --bin desktop` (or let `pnpm desktop:dev` rebuild) and restart the desktop UI. The Core service does not need to restart.

The PNG master is the source of truth; the old C-logo SVG is retired.

## Design provenance

Created with the built-in image_gen tool and subsequently selected explicitly by the user. The selected PNG is committed as `icon-master.png`. Exploratory variants and design prompts remain local and are excluded from the source release under the [source evidence policy](../../../../docs/implementation/source-evidence.md). Icon generation uses the checked-in master and does not require those previews.
