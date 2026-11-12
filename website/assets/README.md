# Website brand assets

`logo-transparent.png` is the website-only mark with a real alpha-transparent background and no app-icon tile. It is black with white facial details for light surfaces; the navbar, download section and footer invert it with CSS for their dark surfaces. Keep its transparent canvas intact.

`favicon.svg`, `favicon-32.png` and `apple-touch-icon.png` are the app icon (`apps/desktop/src-tauri/icons/icon-master.png`): the white sheepdog on its black square, so a browser tab shows the same mark as the dock. The SVG and 32px PNG have rounded corners; the touch icon is square because iOS rounds it. They look the same in light and dark browser themes.

The application icon remains in `apps/desktop/src-tauri/icons`. Running `pnpm icons` only regenerates application assets and does not overwrite these website assets.

## Generation

`logo-transparent.png` was created with built-in imagegen from the approved application icon. The favicons are `icon-master.png` resized with Pillow (Lanczos), with corners rounded at 22% of the side.

Final prompt:

> Website brand mark, transparent PNG, flat vector-style. Use the reference to faithfully reproduce the approved right-facing sheepdog graphic with its upright ear, closed muzzle and curved neck, but INVERT the logo's colors: the WHITE graphic shapes in the reference become solid pure BLACK shapes; the eye and nose become solid WHITE. The large BLACK square background in the reference must be completely absent, replaced by real alpha TRANSPARENCY. The spaces within the upright ear should also be transparent except for the inner ear triangle, which becomes pure BLACK. One isolated BLACK sheepdog logo with WHITE eye and WHITE nose on a transparent background. This black version is for light website backgrounds. No tile or square or frame of any kind. Absolutely clean solid vector fills and smooth edges. No raster texture, noise, distressed edges or random flakes. Preserve the reference's exact silhouette. Entire mark fits centered with a small transparent margin, no cropping. Not a photograph, no text, no shadow. Deliver a clean black logo on actual alpha-transparent canvas.
