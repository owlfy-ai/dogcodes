// Export a reviewed working tree without copying its Git history or local data.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkReleaseInputs, privateOrGeneratedPath } from './check-release-inputs.mjs';

const defaultRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const split = text => text.split('\0').filter(Boolean);
export function prepareSourceSnapshot({ root = defaultRoot, output, reviewedUntracked = [] }) {
  root = realpathSync(root);
  if (!output) throw new Error('Choose a new output directory.');
  let ancestor = resolve(root, output);
  const suffix = [];
  while (!existsSync(ancestor)) {
    suffix.unshift(basename(ancestor));
    ancestor = dirname(ancestor);
  }
  // macOS /var and /private/var (or another parent symlink) must not bypass
  // the inside-repository check or cause an existing source root to be copied.
  output = resolve(realpathSync(ancestor), ...suffix);
  const git = args => execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] });
  checkReleaseInputs({ root, log: () => {} });
  const destination = relative(root, output);
  if (!destination || relative(output, root).split(sep)[0] !== '..' && !isAbsolute(relative(output, root))) {
    throw new Error('The output must not contain the source repository.');
  }
  if (!destination.startsWith(`..${sep}`) && !isAbsolute(destination)) {
    try { git(['check-ignore', '--quiet', '--no-index', '--', destination]); }
    catch { throw new Error('An output inside the repository must be ignored, for example release/private/github-source.'); }
  }
  const manifestPath = `${output}.manifest.json`;
  if (existsSync(output) || existsSync(manifestPath)) throw new Error('Snapshot output already exists; choose a new directory.');
  const inventory = () => {
    const tracked = split(git(['ls-files', '--cached', '-z']));
    const added = split(git(['ls-files', '--others', '--exclude-standard', '-z']));
    const missingReview = added.filter(path => !reviewedUntracked.includes(path));
    if (missingReview.length) throw new Error(`Review these untracked files explicitly before exporting:\n${missingReview.join('\n')}`);
    return [...new Set([...tracked, ...added])].sort();
  };
  const selected = inventory();
  const files = [], deleted = [];
  for (const path of selected) {
    if (privateOrGeneratedPath(path) || path.split('/').some(part => part.toLowerCase() === '.git' || part === '..') || isAbsolute(path)) {
      throw new Error(`Private, generated or unsafe source path: ${path}`);
    }
    const source = resolve(root, path);
    let stat;
    try { stat = lstatSync(source); }
    catch (error) { if (error.code === 'ENOENT') { deleted.push(path); continue; } throw error; }
    if (!stat.isFile() || realpathSync(source) !== source) throw new Error(`Source input must be a regular file without symbolic links: ${path}`);
    const bytes = readFileSync(source);
    files.push({ path, sha256: digest(bytes), bytes: bytes.length, executable: Boolean(stat.mode & 0o111) });
  }
  mkdirSync(dirname(output), { recursive: true });
  mkdirSync(output);
  let wroteManifest = false;
  try {
    for (const file of files) {
      const bytes = readFileSync(resolve(root, file.path));
      if (digest(bytes) !== file.sha256) throw new Error(`Source changed during export: ${file.path}`);
      const target = resolve(output, file.path);
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, bytes, { flag: 'wx', mode: file.executable ? 0o755 : 0o644 });
    }
    // Do not silently export a mixture of files changed by another active editor.
    if (JSON.stringify(inventory()) !== JSON.stringify(selected)) throw new Error('Source inventory changed during export.');
    for (const file of files) {
      if (digest(readFileSync(resolve(root, file.path))) !== file.sha256 || digest(readFileSync(resolve(output, file.path))) !== file.sha256) {
        throw new Error(`Source changed during export: ${file.path}`);
      }
    }
    for (const path of deleted) if (existsSync(resolve(root, path))) throw new Error(`Deleted source input reappeared: ${path}`);
    let baseCommit = null;
    try { baseCommit = git(['rev-parse', 'HEAD']).trim(); } catch { /* A new source repository may not yet have a commit. */ }
    const manifest = { schemaVersion: 1, generatedAt: new Date().toISOString(), historyIncluded: false, baseCommit, reviewedUntracked, deleted, files };
    writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
    wroteManifest = true;
    return { output, manifestPath, fileCount: files.length, manifestSha256: digest(readFileSync(manifestPath)) };
  } catch (error) {
    // Remove only this invocation's new output, never an existing candidate.
    rmSync(output, { recursive: true, force: true });
    if (wroteManifest) rmSync(manifestPath);
    throw error;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2);
    const value = name => { const index = args.indexOf(name); return index < 0 ? undefined : args[index + 1]; };
    const reviewed = value('--reviewed-untracked');
    const reviewedUntracked = reviewed ? JSON.parse(readFileSync(resolve(reviewed), 'utf8')) : [];
    if (!Array.isArray(reviewedUntracked) || reviewedUntracked.some(path => typeof path !== 'string')) throw new Error('Reviewed untracked inputs must be a JSON array of repository-relative paths.');
    console.log(JSON.stringify(prepareSourceSnapshot({ output: value('--output'), reviewedUntracked }), null, 2));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
