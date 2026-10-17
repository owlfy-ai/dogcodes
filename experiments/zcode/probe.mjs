// Credential-opaque M0 probe. Only sessions created here are addressed.
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, isAbsolute } from 'node:path';
import { pathToFileURL } from 'node:url';

export class Peer {
  constructor(executable, cwd) {
    this.pending = new Map(); this.nextId = 1; this.buffer = ''; this.bytes = 0;
    this.notifications = 0; this.clientRequests = 0; this.clientMethods = new Set();
    const env = Object.fromEntries(['PATH','HOME','TMPDIR','TMP','TEMP','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','NO_PROXY'].filter(k => process.env[k]).map(k => [k,process.env[k]]));
    this.child = spawn(executable, ['app-server', '--cwd', cwd, '--mode', 'plan'], { cwd, env, stdio: ['pipe','pipe','pipe'] });
    this.child.stderr.on('data', () => {}); // Never retain vendor logs.
    this.exited = new Promise(resolve => this.child.once('close', resolve));
    this.child.on('error', () => this.fail('spawn_failed'));
    this.child.on('close', () => this.fail('peer_closed'));
    this.child.stdin.on('error', () => this.fail('write_failed'));
    this.child.stdout.setEncoding('utf8');
    this.child.stdout.on('data', text => {
      this.bytes += Buffer.byteLength(text);
      if (this.bytes > 8 * 1024 * 1024) { this.fail('output_limit'); this.child.stdin.end(); return; }
      this.buffer += text;
      if (Buffer.byteLength(this.buffer) > 1024 * 1024) { this.fail('frame_limit'); this.child.stdin.end(); return; }
      let index;
      while ((index = this.buffer.indexOf('\n')) >= 0) {
        const line = this.buffer.slice(0,index); this.buffer = this.buffer.slice(index+1);
        if (!line.trim()) continue;
        let frame;
        try { frame = JSON.parse(line); } catch { this.fail('invalid_json'); return; }
        if (frame.method) {
          if (frame.id !== undefined) {
            this.clientRequests++;
            this.clientMethods.add(['session/requestRuntimePreferences','interaction/requestPermission','interaction/requestUserInput','interaction/requestProviderRuntimeHeaders','interaction/requestOfficialMcpAuthHeaders'].includes(frame.method) ? frame.method : 'unknown');
            // The M0 lifecycle probe must never grant tools or return auth headers.
            const reply = frame.method === 'session/requestRuntimePreferences'
              ? {id:frame.id,result:{nativeSearchEnhancementsEnabled:false,memoryEnabled:false,askUserQuestionAutoResolutionEnabled:false,modelContextBudgetStrategy:'preflight-v1'}}
              : {id:frame.id,error:{code:-32601,message:'Probe does not grant client capabilities'}};
            this.child.stdin.write(JSON.stringify(reply)+'\n');
          } else this.notifications++;
        } else {
          const pending = this.pending.get(frame.id);
          if (!pending) continue;
          this.pending.delete(frame.id); clearTimeout(pending.timer);
          if (frame.error) {
            this.errorCategories = ['model','provider','workspace','runtime','configuration','schema','invalid','bootstrap','registry','missing','permission','initializ','preferences','not found','expected','required'].filter(word => String(frame.error.message).toLowerCase().includes(word));
            pending.reject(new Error(`rpc_${Number.isInteger(frame.error.code)?frame.error.code:'error'}`));
          }
          else pending.resolve(frame.result);
        }
      }
    });
  }
  fail(code) {
    this.failure = code;
    for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(new Error(code)); }
    this.pending.clear();
  }
  request(method, params) {
    if (this.failure) return Promise.reject(new Error(this.failure));
    const id = this.nextId++;
    return new Promise((resolve,reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error('request_timeout')); }, 20000);
      this.pending.set(id,{resolve,reject,timer});
      this.child.stdin.write(JSON.stringify({id,method,params})+'\n');
    });
  }
  async close() {
    this.child.stdin.end();
    const timer = setTimeout(() => this.child.kill('SIGTERM'), 5000);
    const lastResort = setTimeout(() => this.child.kill('SIGKILL'), 7000);
    await this.exited; clearTimeout(timer); clearTimeout(lastResort);
  }
}

async function main() {
  const executable = process.argv[2];
  if (!executable || !isAbsolute(executable) || process.argv.length !== 3) throw new Error('usage_absolute_executable');
  const versionResult = spawnSync(executable,['--version'],{timeout:5000,maxBuffer:65536,encoding:'utf8'});
  const version = versionResult.status === 0 ? versionResult.stdout.match(/\b\d+\.\d+\.\d+\b/)?.[0] : undefined;
  if (!version) throw new Error('version_failed');
  const cwd = await mkdtemp(join(tmpdir(),'dogcodes-zcode-m0-'));
  const peer = new Peer(executable,cwd);
  const report = {version, modelTurns:0, checks:{}};
  try {
    try { await peer.request('initialize',{}); report.checks.initialize = 'accepted'; }
    catch (error) { report.checks.initialize = error.message; }
    const created = await peer.request('session/create',{
      workspace:{workspacePath:cwd,workspaceKey:cwd},mode:'plan',persistence:'deferred',
      titleGenerationEnabled:false,mcpServers:[],toolAllowlist:[],
    });
    const snapshot = created.snapshot ?? created;
    const id = snapshot.session?.sessionId ?? created.sessionId;
    report.checks.create = {hasSessionId:typeof id==='string',protocol:snapshot.protocol?.version ?? null};
    if (typeof id !== 'string') throw new Error('missing_session_identity');
    const subscription = await peer.request('session/subscribe',{sessionId:id,deliveryKind:'desktop-continuous',includeSnapshot:false});
    report.checks.subscribe = {hasSequence:Number.isInteger(subscription.eventSeq)};
    await peer.request('session/stop',{sessionId:id});
    report.checks.idleStop = 'accepted';
    const closed = await peer.request('session/close',{sessionId:id,expectedPersistence:'deferred'});
    report.checks.close = closed.closed === true;
  } catch (error) { report.failure = /^[a-z_0-9-]+$/.test(error.message) ? error.message : 'probe_failed'; }
  finally { await peer.close(); }
  report.notifications = peer.notifications; report.clientRequests = peer.clientRequests;
  report.clientMethods = [...peer.clientMethods];
  if (report.failure) report.errorCategories = peer.errorCategories;
  console.log(JSON.stringify(report,null,2));
  if (report.failure) process.exitCode = 1;
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(() => { console.error('probe_failed'); process.exitCode=1; });
}
