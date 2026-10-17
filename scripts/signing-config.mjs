// This allowlist is intentionally narrower than Tauri's packaging schema.
// Local signing inputs must not silently add resources or change installers.
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const nullableString = value => value === null || typeof value === 'string';
const properties = (value, allowed) => object(value) && Object.keys(value).every(key => allowed.includes(key));
export function validateSigningConfig(config) {
  const fail = () => { throw new Error('Signing configuration must contain only supported signing fields in bundle.macOS and bundle.windows.'); };
  if (!properties(config, ['$schema', 'bundle']) || !properties(config.bundle, ['macOS', 'windows'])
      || ('$schema' in config && typeof config.$schema !== 'string')) fail();
  const mac = config.bundle.macOS;
  if (mac !== undefined) {
    if (!properties(mac, ['signingIdentity', 'hardenedRuntime', 'providerShortName', 'entitlements'])) fail();
    for (const [key, value] of Object.entries(mac)) {
      if (key === 'hardenedRuntime' ? typeof value !== 'boolean' : !nullableString(value)) fail();
    }
  }
  const windows = config.bundle.windows;
  if (windows !== undefined) {
    if (!properties(windows, ['certificateThumbprint', 'digestAlgorithm', 'timestampUrl', 'tsp', 'signCommand'])) fail();
    for (const [key, value] of Object.entries(windows)) {
      if (key === 'tsp') { if (typeof value !== 'boolean') fail(); }
      else if (key === 'signCommand' && object(value)) {
        if (!properties(value, ['cmd', 'args']) || typeof value.cmd !== 'string' || !value.cmd.trim()
            || !Array.isArray(value.args) || !value.args.every(arg => typeof arg === 'string')) fail();
      } else if (!nullableString(value)) fail();
    }
  }
  return config;
}
export function parseSigningConfig(text) {
  let config;
  try { config = JSON.parse(text); }
  catch { throw new Error('Invalid signing JSON format; file contents are not shown.'); }
  return validateSigningConfig(config);
}
export function validateBlankSigningExample(text) {
  const config = parseSigningConfig(text);
  const mac = config.bundle.macOS, windows = config.bundle.windows;
  const exact = (value, keys) => object(value) && keys.length === Object.keys(value).length && keys.every(key => key in value);
  if (!exact(mac, ['signingIdentity']) || mac.signingIdentity !== null
      || !exact(windows, ['certificateThumbprint', 'digestAlgorithm', 'timestampUrl', 'signCommand'])
      || windows.certificateThumbprint !== null || windows.timestampUrl !== null || windows.signCommand !== null
      || windows.digestAlgorithm !== 'sha256'
      || (config.$schema !== undefined && config.$schema !== 'https://schema.tauri.app/config/2')) {
    throw new Error('The public signing example must contain only the blank signing fields and SHA-256 digest setting.');
  }
}
