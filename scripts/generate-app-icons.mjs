import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const icons = join(root, "apps/desktop/src-tauri/icons");
const output = mkdtempSync(join(tmpdir(), "dogcodes-icons-"));
const source = join(icons, "icon-master.png");

function generate(input, destination) {
  execFileSync("pnpm", [
    "--filter", "@dogcodes/desktop", "exec", "tauri", "icon",
    input, "--output", destination,
  ], { cwd: root, stdio: "inherit" });
}

try {
  const standard = join(output, "standard");
  const windows = join(output, "windows");
  const macos = join(output, "macos");
  generate(source, standard);
  // Keep PNG assets square for Linux and the in-app logo.
  for (const name of [
    "32x32.png", "64x64.png", "128x128.png", "128x128@2x.png",
    "icon.png",
  ]) {
    copyFileSync(join(standard, name), join(icons, name));
  }
  // Website marks have transparent backgrounds and are maintained separately.

  const png = readFileSync(source).toString("base64");
  // Windows displays the ICO as-is. Bake transparent rounded corners into
  // every size rather than relying on the shell to mask the square artwork.
  const windowsSource = join(output, "windows-icon.svg");
  writeFileSync(windowsSource, `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs><clipPath id="tile"><rect width="1024" height="1024" rx="230"/></clipPath></defs>
  <image width="1024" height="1024" xlink:href="data:image/png;base64,${png}" clip-path="url(#tile)"/>
</svg>`);
  generate(windowsSource, windows);
  copyFileSync(join(windows, "icon.ico"), join(icons, "icon.ico"));

  // ICNS is displayed as-is in the Dock. Give macOS a rounded tile with
  // transparent padding.
  const macosSource = join(output, "macos-icon.svg");
  writeFileSync(macosSource, `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs><clipPath id="tile"><rect x="100" y="100" width="824" height="824" rx="185"/></clipPath></defs>
  <image x="100" y="100" width="824" height="824" xlink:href="data:image/png;base64,${png}" clip-path="url(#tile)"/>
</svg>`);
  generate(macosSource, macos);
  copyFileSync(join(macos, "icon.icns"), join(icons, "icon.icns"));
} finally {
  rmSync(output, { recursive: true, force: true });
}
