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
  const macos = join(output, "macos");
  generate(source, standard);
  // Copy Tauri's generated assets without cropping or changing the selected artwork.
  for (const name of [
    "32x32.png", "64x64.png", "128x128.png", "128x128@2x.png",
    "icon.png", "icon.ico",
  ]) {
    copyFileSync(join(standard, name), join(icons, name));
  }
  copyFileSync(join(standard, "icon.png"), join(root, "website/assets/logo.png"));

  // ICNS is displayed as-is in the Dock. Give macOS a rounded tile with
  // transparent padding while keeping the website and other OS icons square.
  const png = readFileSync(source).toString("base64");
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
