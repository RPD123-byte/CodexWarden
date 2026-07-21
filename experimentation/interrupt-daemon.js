// Real-time Codex interrupt daemon (the facial-expression daemon's control core).
// Connects to the managed app-server daemon as a second client, auto-subscribes to
// every new GUI thread via the global thread/started push, tracks active turns from
// pushed turn/started, and interrupts on demand — ZERO polling, no shim.
//
// Demo mode: --interrupt-after <ms> interrupts each turn that long after it starts,
// simulating a facial-expression threshold trip. In production, call interrupt()
// from your real trigger instead.
const WebSocket = require('ws');
const os = require('os'), path = require('path');

const SOCK = path.join(os.homedir(), '.codex/app-server-control/app-server-control.sock');
const afterIdx = process.argv.indexOf('--interrupt-after');
const INTERRUPT_AFTER = afterIdx > -1 ? Number(process.argv[afterIdx + 1]) : null;
const t0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(2)}s`, ...a);

const ws = new WebSocket(`ws+unix://${SOCK}:/rpc`, { perMessageDeflate: false });
let id = 1; const P = new Map();
const activeTurns = new Map();   // threadId -> turnId  (the live interrupt targets)
const subscribed = new Set();
const call = (m, p = {}) => { const i = id++; ws.send(JSON.stringify({ jsonrpc: '2.0', id: i, method: m, params: p })); return new Promise((res, rej) => P.set(i, { res, rej })); };

// PUBLIC: interrupt the active turn on a thread (or all active turns if none given).
async function interrupt(threadId) {
  const targets = threadId ? [[threadId, activeTurns.get(threadId)]] : [...activeTurns.entries()];
  for (const [tid, turnId] of targets) {
    if (!turnId) continue;
    try { const r = await call('turn/interrupt', { threadId: tid, turnId }); log('INTERRUPT', String(tid).slice(-8), String(turnId).slice(-8), '→', JSON.stringify(r)); }
    catch (e) { log('interrupt error:', e.message); }
  }
}
module.exports = { interrupt };

async function subscribe(threadId) {
  if (subscribed.has(threadId)) return;
  for (let i = 0; i < 8; i++) {
    try {
      await call('thread/resume', { threadId });
      subscribed.add(threadId);
      log('subscribed', String(threadId).slice(-8));
      // Close the race: a turn may have started before we finished subscribing.
      const read = await call('thread/read', { threadId, includeTurns: true }).catch(() => null);
      const active = (read?.thread?.turns || []).find(t => t.status === 'inProgress');
      if (active && !activeTurns.has(threadId)) {
        activeTurns.set(threadId, active.id);
        log('caught in-flight turn on subscribe', String(threadId).slice(-8), String(active.id).slice(-8));
        if (INTERRUPT_AFTER != null) setTimeout(() => interrupt(threadId), INTERRUPT_AFTER);
      }
      return;
    } catch { await new Promise(r => setTimeout(r, 250)); }
  }
}

ws.on('open', async () => {
  await call('initialize', { clientInfo: { name: 'facial-interrupt-daemon', title: 'daemon', version: '1.0.0' } });
  ws.send(JSON.stringify({ jsonrpc: '2.0', method: 'initialized', params: {} }));
  log('READY — watching all GUI threads (interrupt-after=' + INTERRUPT_AFTER + 'ms)');
});

ws.on('message', (buf) => {
  let m; try { m = JSON.parse(buf.toString()); } catch { return; }
  if (m.id !== undefined && P.has(m.id)) { const x = P.get(m.id); P.delete(m.id); return m.error ? x.rej(new Error(JSON.stringify(m.error))) : x.res(m.result); }
  const p = m.params || {};
  if (m.method === 'thread/started' && p.thread?.id) subscribe(p.thread.id);
  if (m.method === 'turn/started') {
    const threadId = p.threadId, turnId = p.turn?.id;
    if (threadId && turnId) {
      activeTurns.set(threadId, turnId);
      log('turn/started', String(threadId).slice(-8), String(turnId).slice(-8));
      if (INTERRUPT_AFTER != null) setTimeout(() => interrupt(threadId), INTERRUPT_AFTER);
    }
  }
  if (m.method === 'turn/completed') {
    const threadId = p.threadId, st = p.turn?.status ?? p.status;
    if (threadId) { log('turn/completed', String(threadId).slice(-8), 'status=', st); activeTurns.delete(threadId); }
  }
});
ws.on('error', (e) => { log('ERR', e.message); process.exit(1); });
setTimeout(() => process.exit(0), 120000);
