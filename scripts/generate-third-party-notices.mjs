// Writes THIRD_PARTY_NOTICES.md: the license texts of third-party code that ships in the
// desktop app. It walks runtime dependency graphs only — Rust crates resolved for the
// release targets, and the frontend's runtime npm packages — and copies each package's
// own license files. Nothing is guessed: a package without license files is listed as such.
// Usage: node scripts/generate-third-party-notices.mjs [--check]
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const output = join(root, 'THIRD_PARTY_NOTICES.md');
// Keep in sync with the installer matrix in .github/workflows/desktop.yml.
export const releaseTargets = ['x86_64-pc-windows-msvc', 'aarch64-apple-darwin', 'x86_64-apple-darwin', 'x86_64-unknown-linux-gnu'];
// Build tooling listed under the desktop app's dependencies; none of it is in the bundle.
export const npmBuildOnly = new Set(['vite', '@vitejs/plugin-react']);
const licenseFile = /^(licen[cs]e|copying|notice|unlicense|copyright)([-._].*)?$/i;

export function licenseFiles(directory) {
  let names;
  try { names = readdirSync(directory); } catch { return []; }
  return names
    .filter(name => licenseFile.test(name) && statSync(join(directory, name)).isFile())
    .sort()
    .map(name => ({ name, text: readFileSync(join(directory, name), 'utf8').replace(/\r\n/g, '\n').trim() }))
    .filter(file => file.text);
}

function rustPackages() {
  const found = new Map();
  for (const target of releaseTargets) {
    const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--format-version', '1', '--locked', '--filter-platform', target], {
      cwd: root, maxBuffer: 256 * 1024 * 1024, encoding: 'utf8',
    }));
    const byId = new Map(metadata.packages.map(p => [p.id, p]));
    const nodes = new Map(metadata.resolve.nodes.map(n => [n.id, n]));
    const start = metadata.packages.find(p => p.name === 'desktop' && !p.source);
    if (!start) throw new Error('The desktop package was not found in cargo metadata.');
    const queue = [start.id];
    const seen = new Set(queue);
    while (queue.length) {
      const id = queue.shift();
      const pkg = byId.get(id);
      // Workspace crates are this project's own code; walk through them without listing.
      if (pkg.source) found.set(`${pkg.name}@${pkg.version}`, pkg);
      for (const dep of nodes.get(id)?.deps ?? []) {
        // Normal (runtime) edges only: build scripts and dev-dependencies are not shipped.
        if (!dep.dep_kinds.some(kind => kind.kind === null) || seen.has(dep.pkg)) continue;
        seen.add(dep.pkg);
        queue.push(dep.pkg);
      }
    }
  }
  return [...found.values()].map(pkg => ({
    ecosystem: 'Rust crate', name: pkg.name, version: pkg.version,
    license: pkg.license ?? (pkg.license_file ? `see ${pkg.license_file}` : 'not declared'),
    repository: pkg.repository ?? pkg.homepage ?? '',
    files: licenseFiles(dirname(pkg.manifest_path)),
  }));
}

function packageDirectory(fromDirectory, name) {
  // Node's lookup: the nearest node_modules/<name> walking up from the dependent.
  for (let dir = fromDirectory; ; dir = dirname(dir)) {
    const candidate = join(dir, 'node_modules', name);
    if (existsSync(join(candidate, 'package.json'))) return realpathSync(candidate);
    if (dirname(dir) === dir) return null;
  }
}

function dependencyDirectory(packageDir, ownName, name) {
  // In the pnpm store a package lives at .../node_modules/<ownName>; its deps are linked
  // as siblings in that same node_modules directory.
  const modules = ownName.startsWith('@') ? dirname(dirname(packageDir)) : dirname(packageDir);
  const candidate = join(modules, name);
  return existsSync(join(candidate, 'package.json')) ? realpathSync(candidate) : packageDirectory(packageDir, name);
}

function npmPackages() {
  const app = join(root, 'apps/desktop');
  const manifest = JSON.parse(readFileSync(join(app, 'package.json'), 'utf8'));
  const found = new Map();
  const queue = [];
  const visit = (directory, isWorkspace) => {
    const pkg = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
    const key = `${pkg.name}@${pkg.version}`;
    if (found.has(key)) return;
    found.set(key, isWorkspace ? null : {
      ecosystem: 'npm package', name: pkg.name, version: pkg.version,
      license: typeof pkg.license === 'string' ? pkg.license : pkg.license?.type ?? 'not declared',
      repository: typeof pkg.repository === 'string' ? pkg.repository : pkg.repository?.url ?? pkg.homepage ?? '',
      files: licenseFiles(directory),
    });
    queue.push(directory);
  };
  for (const name of Object.keys(manifest.dependencies ?? {})) {
    if (npmBuildOnly.has(name)) continue;
    const directory = packageDirectory(app, name);
    if (!directory) throw new Error(`Runtime dependency ${name} is not installed; run pnpm install first.`);
    visit(directory, !directory.includes(`${join('node_modules', '.pnpm')}`));
  }
  while (queue.length) {
    const directory = queue.shift();
    const pkg = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
    for (const name of Object.keys({ ...pkg.dependencies, ...pkg.optionalDependencies })) {
      const dep = dependencyDirectory(directory, pkg.name, name);
      // Optional platform packages that were not installed are not part of this build.
      if (dep) visit(dep, !dep.includes(`${join('node_modules', '.pnpm')}`));
    }
  }
  return [...found.values()].filter(Boolean);
}

export function render(packages) {
  const sorted = [...packages].sort((a, b) => a.ecosystem.localeCompare(b.ecosystem) || a.name.localeCompare(b.name) || a.version.localeCompare(b.version, undefined, { numeric: true }));
  // Identical license texts are printed once, with every package that ships them.
  const texts = new Map();
  for (const pkg of sorted) {
    for (const file of pkg.files) {
      const hash = createHash('sha256').update(file.text).digest('hex');
      if (!texts.has(hash)) texts.set(hash, { text: file.text, users: [] });
      texts.get(hash).users.push(`${pkg.name} ${pkg.version} (${file.name})`);
    }
  }
  const missing = sorted.filter(pkg => !pkg.files.length);
  const lines = [
    '# Third-party notices',
    '',
    'dogcodes includes the third-party software listed below. Each component is used under the',
    'license its authors published with it; the license texts follow. dogcodes itself is licensed',
    'under the Apache License 2.0 (see LICENSE).',
    '',
    'Generated by `scripts/generate-third-party-notices.mjs` from the runtime dependency graphs',
    `of the desktop app (Rust targets: ${releaseTargets.join(', ')}; frontend runtime npm packages).`,
    '',
    `## Components (${sorted.length})`,
    '',
    '| Component | Version | Ecosystem | License | Source |',
    '| --- | --- | --- | --- | --- |',
    ...sorted.map(p => `| ${p.name} | ${p.version} | ${p.ecosystem} | ${p.license.replace(/\|/g, '\\|')} | ${p.repository.replace(/^git\+/, '')} |`),
    '',
  ];
  if (missing.length) {
    lines.push('## Components without bundled license files', '',
      'These packages did not include a license file in their published sources. Their declared',
      'license is listed above; the corresponding standard license text applies.', '',
      ...missing.map(p => `- ${p.name} ${p.version} — ${p.license}`), '');
  }
  lines.push('## License texts', '');
  for (const { text, users } of texts.values()) {
    lines.push('---', '', `Used by: ${users.join(', ')}`, '', '```text', text.replace(/```/g, '`​``'), '```', '');
  }
  return `${lines.join('\n').trimEnd()}\n`;
}

function main() {
  const check = process.argv.includes('--check');
  const content = render([...rustPackages(), ...npmPackages()]);
  if (check) {
    const current = existsSync(output) ? readFileSync(output, 'utf8') : '';
    if (current !== content) {
      console.error('THIRD_PARTY_NOTICES.md is out of date. Run: pnpm notices:generate');
      process.exitCode = 1;
      return;
    }
    console.log('THIRD_PARTY_NOTICES.md is up to date.');
    return;
  }
  writeFileSync(output, content);
  console.log(`Wrote THIRD_PARTY_NOTICES.md (${content.length} bytes).`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
