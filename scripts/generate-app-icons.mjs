import { copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const icons = join(root, "apps/desktop/src-tauri/icons");
const output = mkdtempSync(join(tmpdir(), "dogcodes-icons-"));

try {
  execFileSync("pnpm", [
    "--filter", "@dogcodes/desktop", "exec", "tauri", "icon",
    join(icons, "icon-master.png"), "--output", output,
  ], { cwd: root, stdio: "inherit" });
  // Keep the generated alpha margins; cropping makes the Dock icon oversized.
  for (const name of [
    "32x32.png", "64x64.png", "128x128.png", "128x128@2x.png",
    "icon.png", "icon.icns", "icon.ico",
  ]) {
    copyFileSync(join(output, name), join(icons, name));
  }
} finally {
  rmSync(output, { recursive: true, force: true });
}
