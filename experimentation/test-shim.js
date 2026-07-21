// Proves the shim: harness acts as the "GUI" (parent), spawning the SHIM
// instead of codex. It runs a normal initialize/thread/start/turn/start.
// Separately, a control-socket client injects turn/interrupt mid-turn.
// Success = parent sees turn/completed status "interrupted", stream intact.

const { spawn } = require('child_process');
const net = require('net');
const path = require('path');
const readline = require('readline');

const DIR = __dirname;
const SOCK = path.join(DIR, 'shim.sock');
const REAL = process.argv[2] || 'codex'; // pass DESKTOP_BINARY to use the app's codex
const t0 = Date.now();
const log = (tag, ...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(2)}s [${tag}]`, ...a);

// Parent speaks JSON-RPC to the shim's stdio (exactly like the GUI would).
const shim = spawn('node', [path.join(DIR, 'shim-codex.js'), 'app-server'], {
  stdio: ['pipe', 'pipe', 'inherit'],
  env: { ...process.env, SHIM_REAL_CODEX: REAL, SHIM_DIR: DIR },
});

let nextId = 1;
const pending = new Map();
const send = (method, params) => {
  const id = nextId++;
  shim.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  return new Promise((res) => pending.set(id, res));
};

let threadId, turnId, interruptedSeen = false;
const rl = readline.createInterface({ input: shim.stdout });
rl.on('line', (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.id !== undefined && pending.has(msg.id)) { pending.get(msg.id)(msg.result); pending.delete(msg.id); return; }
  if (msg.method === 'turn/started') { turnId = msg.params?.turn?.id ?? msg.params?.turnId; log('parent-sees', 'turn/started', turnId); }
  if (msg.method === 'turn/completed') {
    const status = msg.params?.turn?.status ?? msg.params?.status;
    log('parent-sees', 'turn/completed status=', status);
    interruptedSeen = status === 'interrupted';
    finish();
  }
});

let done = false;
function finish() {
  if (done) return; done = true;
  log('RESULT', interruptedSeen ? 'PARENT SAW INTERRUPTED ✅' : 'did not see interrupted ❌');
  shim.kill(); process.exit(interruptedSeen ? 0 : 1);
}

(async () => {
  await new Promise((r) => setTimeout(r, 500)); // let shim bind socket
  await send('initialize', { clientInfo: { name: 'fake-gui', title: 'Fake GUI', version: '0.1.0' } });
  log('parent', 'initialized');
  const th = await send('thread/start', { cwd: DIR, ephemeral: false });
  threadId = th?.thread?.id ?? th?.threadId ?? th?.id;
  log('parent', 'thread', threadId);
  send('turn/start', { threadId, input: [{ type: 'text', text: 'Write a very long, detailed 2500-word essay about the history of computing. Take your time.' }] });

  // The "daemon": connect to the shim control socket and interrupt after 5s.
  setTimeout(() => {
    const c = net.connect(SOCK, () => log('daemon', 'connected to shim.sock'));
    const crl = readline.createInterface({ input: c });
    crl.on('line', (l) => {
      const m = JSON.parse(l);
      if (m.type === 'hello') { log('daemon', 'hello; sniffed state=', JSON.stringify(m.state)); }
      if (m.type === 'injected') { log('daemon', 'INJECTED turn/interrupt', JSON.stringify(m.frame.params)); }
      if (m.type === 'error') { log('daemon', 'inject-error', m.error); }
    });
    setTimeout(() => c.write(JSON.stringify({ type: 'interrupt' }) + '\n'), 300);
  }, 5000);

  setTimeout(() => { log('timeout', '90s'); finish(); }, 90000);
})();
