// Appended to quota-reader.cjs to reuse native credential decoding. Output is
// private process-pipe data for the native server, never a persisted artifact.
function runtimeFrom(config, input) {
  const ref = input.model;
  if (!ref || typeof ref.providerId !== 'string' || typeof ref.modelId !== 'string') throw new Error('model_unavailable');
  const configured = config.provider?.[ref.providerId];
  if (!configured || configured.enabled === false || !Object.hasOwn(configured.models || {}, ref.modelId)) throw new Error('model_unavailable');
  if (!['anthropic', 'openai', 'openai-compatible'].includes(configured.kind)) throw new Error('provider_unsupported');
  const options = configured.options || {};
  const provider = { providerId: ref.providerId, kind: configured.kind, source: 'user',
    models: [{ ...(input.definition || {}), modelId: ref.modelId }] };
  if (options.baseURL != null) {
    const url = new URL(options.baseURL);
    if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password) throw new Error('provider_unsupported');
    provider.baseURL = options.baseURL;
  }
  if (options.apiKey != null) provider.apiKey = { source: 'inline', value: credential(options.apiKey) };
  if (options.headers != null) provider.headers = options.headers;
  const extra = { ...options }; delete extra.apiKey; delete extra.baseURL; delete extra.headers;
  if (Object.keys(extra).length) provider.providerOptions = extra;
  return { revision: `native-config:${crypto.randomUUID()}`, generatedAt: Date.now(), model: ref, provider,
    ...(input.thoughtLevel ? { thoughtLevel: input.thoughtLevel } : {}) };
}
function runtimeMain() {
  try {
    const file = path.join(os.homedir(), '.zcode', 'cli', 'config.json');
    if (fs.statSync(file).size > LIMIT) throw new Error('config_unavailable');
    const input = fs.readFileSync(0, 'utf8');
    if (Buffer.byteLength(input) > LIMIT) throw new Error('input_unavailable');
    process.stdout.write(JSON.stringify(runtimeFrom(JSON.parse(fs.readFileSync(file, 'utf8')), JSON.parse(input))));
  } catch { process.exitCode = 1; }
}
module.exports.runtimeFrom = runtimeFrom;
