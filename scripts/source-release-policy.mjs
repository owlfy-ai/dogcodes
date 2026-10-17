// Explicitly reviewed protocol observations; new fixtures need their own review.
export const publicExperimentFixtures = new Set([
  'experiments/acp/fixtures/stable-v1-offline-contract.json',
  'experiments/codex-app-server/fixtures/initialize-handshakes.json',
  'experiments/codex-app-server/fixtures/persistent-resume.json',
  'experiments/codex-app-server/fixtures/schema-fingerprints.json',
]);
export function localArtifactPath(path) {
  const lower = path.toLowerCase();
  if (lower.startsWith('output/') || lower.startsWith('artifacts/')) return true;
  return lower.startsWith('experiments/') && /\.(jsonl?|log|txt|png|jpe?g|webp|svg|html|zip)$/.test(lower)
    && !publicExperimentFixtures.has(path);
}

// Only reviewed, build-required source material belongs in release/.
export function localReleasePath(path) {
  const lower = path.toLowerCase();
  if (!lower.startsWith('release/')) return false;
  return !['release/readme.md', 'release/.env.example', 'release/tauri.signing.example.json'].includes(lower);
}
