#!/usr/bin/env node
// Man-in-the-middle shim for the codex app-server.
// Sits at the path the parent (ChatGPT GUI, or our test harness) execs.
// - Forwards parent<->realcodex stdio byte-for-byte (transparent).
// - Tees every JSON-RPC line to a log + a control socket (observation).
// - Accepts injected JSON-RPC frames on the control socket and writes them
//   into real codex's stdin (e.g. turn/interrupt) using a high id range so
//   they never collide with the parent's ids.
//
// Env:
//   SHIM_REAL_CODEX  path to the genuine codex binary (required)
//   SHIM_DIR         dir for shim.sock + shim-log.ndjson (default: script dir)

const { spawn } = require('child_process');
const net = require('net');
const fs = require('fs');
const path = require('path');
const readline = require('readline');

const REAL = process.env.SHIM_REAL_CODEX;
const DIR = process.env.SHIM_DIR || __dirname;
const SOCK = path.join(DIR, `shim-${process.pid}.sock`); // per-PID: no collision across multiple spawns
const REGISTRY = path.join(DIR, 'shim-registry.ndjson');
const LOG = path.join(DIR, 'shim-log.ndjson');

if (!REAL) { process.stderr.write('shim: SHIM_REAL_CODEX not set\n'); process.exit(2); }

const logStream = fs.createWriteStream(LOG, { flags: 'a' });
const logEvent = (dir, obj) => {
  try { logStream.write(JSON.stringify({ t: Date.now(), pid: process.pid, argv: process.argv.slice(2).join(' '), dir, ...obj }) + '\n'); } catch {}
};

// State we sniff from the stream so the daemon can interrupt with no bookkeeping.
const state = { threadId: null, turnId: null, initialized: false, turns: {} };

// Spawn the genuine app-server with identical argv/stdio.
const real = spawn(REAL, process.argv.slice(2), { stdio: ['pipe', 'pipe', 'inherit'] });
real.on('exit', (code, sig) => { logEvent('sys', { event: 'real-exit', code, sig }); process.exit(code ?? 0); });

// --- parent stdin -> real stdin (tee) ---
const parentRL = readline.createInterface({ input: process.stdin });
parentRL.on('line', (line) => {
  sniff('c2s', line);
  real.stdin.write(line + '\n');
});
process.stdin.on('end', () => real.stdin.end());

// --- real stdout -> parent stdout (tee) ---
const realRL = readline.createInterface({ input: real.stdout });
realRL.on('line', (line) => {
  sniff('s2c', line);
  process.stdout.write(line + '\n');
  broadcast(line);
});

const rawStream = fs.createWriteStream(path.join(DIR, 'shim-raw.ndjson'), { flags: 'a' });
function sniff(dir, line) {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  try { rawStream.write(JSON.stringify({ t: Date.now(), pid: process.pid, dir, msg }) + '\n'); } catch {}
  const isErr = msg.error !== undefined;
  const rec = { method: msg.method, id: msg.id, hasResult: msg.result !== undefined };
  // Capture full payload for the things we care about diagnosing.
  if (msg.method === 'turn/started' || msg.method === 'turn/completed' || isErr) rec.full = msg;
  logEvent(dir, rec);

  // Track active (threadId, turnId) pairs ATOMICALLY from turn/started.
  if (msg.method === 'turn/started') {
    const p = msg.params || {};
    const threadId = p.threadId || p.thread?.id || state.threadId;
    const turnId = p.turn?.id || p.turnId;
    if (threadId && turnId) { state.threadId = threadId; state.turnId = turnId; state.turns[turnId] = threadId; }
  } else if (msg.method === 'turn/completed') {
    const p = msg.params || {};
    const turnId = p.turn?.id || p.turnId;
    if (turnId) delete state.turns[turnId];
    if (turnId === state.turnId) state.turnId = null;
  } else {
    const p = msg.params || msg.result || {};
    const tId = p.threadId || p.thread?.id;
    if (tId) state.threadId = tId;
  }
}

// --- control socket: observers + injectors ---
try { fs.unlinkSync(SOCK); } catch {}
const clients = new Set();
let injectId = 1_000_000; // high range, cannot collide with GUI ids

const server = net.createServer((sock) => {
  clients.add(sock);
  sock.write(JSON.stringify({ type: 'hello', state }) + '\n');
  const rl = readline.createInterface({ input: sock });
  rl.on('line', (line) => {
    let cmd; try { cmd = JSON.parse(line); } catch { return; }
    if (cmd.type === 'state') { sock.write(JSON.stringify({ type: 'state', state }) + '\n'); return; }
    if (cmd.type === 'interrupt') {
      const threadId = cmd.threadId || state.threadId;
      const turnId = cmd.turnId || state.turnId;
      if (!threadId || !turnId) { sock.write(JSON.stringify({ type: 'error', error: 'no active turn known', state }) + '\n'); return; }
      const frame = { jsonrpc: '2.0', id: injectId++, method: 'turn/interrupt', params: { threadId, turnId } };
      real.stdin.write(JSON.stringify(frame) + '\n');
      logEvent('inject', { method: 'turn/interrupt', threadId, turnId, id: frame.id });
      sock.write(JSON.stringify({ type: 'injected', frame }) + '\n');
    }
    if (cmd.type === 'raw' && cmd.frame) {
      real.stdin.write(JSON.stringify(cmd.frame) + '\n');
      logEvent('inject', { raw: true, id: cmd.frame.id, method: cmd.frame.method });
    }
  });
  sock.on('close', () => clients.delete(sock));
  sock.on('error', () => clients.delete(sock));
});
server.listen(SOCK, () => {
  logEvent('sys', { event: 'shim-up', sock: SOCK, real: REAL });
  try { fs.appendFileSync(REGISTRY, JSON.stringify({ t: Date.now(), pid: process.pid, sock: SOCK, argv: process.argv.slice(2) }) + '\n'); } catch {}
});
process.on('exit', () => { try { fs.unlinkSync(SOCK); } catch {} });

function broadcast(line) {
  for (const c of clients) { try { c.write(JSON.stringify({ type: 'traffic', line }) + '\n'); } catch {} }
}
