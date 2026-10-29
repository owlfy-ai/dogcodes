# Website brand assets

`logo-transparent.png` is the website-only mark with a real alpha-transparent background and no app-icon tile. It is black with white facial details for light surfaces; the navbar, download section and footer invert it with CSS for their dark surfaces. Keep its transparent canvas intact.

`favicon.svg` embeds a 64px PNG export of the same mark and inverts it for dark browser themes. It has no background tile and no external image dependency.

The application icon remains in `apps/desktop/src-tauri/icons`. Running `pnpm icons` only regenerates application assets and does not overwrite these website assets.

## Generation

Created with built-in imagegen from the approved application icon. The favicon PNG was resized with the installed Tauri icon CLI (`--png 64`); the generated alpha channel was preserved.

Final prompt:

> Website brand mark, transparent PNG, flat vector-style. Use the reference to faithfully reproduce the approved right-facing sheepdog graphic with its upright ear, closed muzzle and curved neck, but INVERT the logo's colors: the WHITE graphic shapes in the reference become solid pure BLACK shapes; the eye and nose become solid WHITE. The large BLACK square background in the reference must be completely absent, replaced by real alpha TRANSPARENCY. The spaces within the upright ear should also be transparent except for the inner ear triangle, which becomes pure BLACK. One isolated BLACK sheepdog logo with WHITE eye and WHITE nose on a transparent background. This black version is for light website backgrounds. No tile or square or frame of any kind. Absolutely clean solid vector fills and smooth edges. No raster texture, noise, distressed edges or random flakes. Preserve the reference's exact silhouette. Entire mark fits centered with a small transparent margin, no cropping. Not a photograph, no text, no shadow. Deliver a clean black logo on actual alpha-transparent canvas.
