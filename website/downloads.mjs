// Public client downloads come from the latest published GitHub Release.
// Draft assets are not public, so the site keeps the previous links until publication.

export function signedClientDownloads(assets = []) {
  const one = suffix => {
    const found = assets.filter(asset => asset?.name?.endsWith(suffix) && asset.browser_download_url);
    return found.length === 1 ? found[0].browser_download_url : null;
  };
  return {
    macArm: one('-macOS-AppleSilicon.dmg'),
    macIntel: one('-macOS-Intel.dmg'),
    windows: one('-Windows-x64-Setup.exe'),
  };
}

export function chooseDownload(downloads, { os, macArch = 'arm' } = {}) {
  if (!downloads) return null;
  if (os === 'win') return downloads.windows;
  if (os === 'mac') return macArch === 'intel' ? downloads.macIntel || downloads.macArm : downloads.macArm || downloads.macIntel;
  return null;
}
