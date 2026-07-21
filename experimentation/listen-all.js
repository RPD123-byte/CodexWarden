// Persistent second client on the daemon. Logs EVERY inbound notification.
// Modes:
//   (default)         pure listen — do the GUI create a new chat, see if any global event arrives
//   --auto-subscribe  poll thread/list every 1.5s; when a NEW thread appears, thread/resume it
//                     to subscribe, so its turn/started etc. get pushed in real time.
const WebSocket = require('ws');
const os = require('os'), path = require('path');
const SOCK = path.join(os.homedir(), '.codex/app-server-control/app-server-control.sock');
const ws = new WebSocket(`ws+unix://${SOCK}:/rpc`, { perMessageDeflate: false });
const AUTO = process.argv.includes('--auto-subscribe');
const t0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(2)}s`, ...a);
let id = 1; const P = new Map();
const known = new Set();
const call = (m, p = {}) => { const i = id++; ws.send(JSON.stringify({ jsonrpc: '2.0', id: i, method: m, params: p })); return new Promise((res, rej) => P.set(i, { res, rej })); };

ws.on('open', async () => {
  await call('initialize', { clientInfo: { name: 'listener', title: 'l', version: '0.1.0' } });
  ws.send(JSON.stringify({ jsonrpc: '2.0', method: 'initialized', params: {} }));
  log('LISTENING (auto-subscribe=' + AUTO + ')');
  const list = await call('thread/list', { limit: 20 });
  for (const t of list.data || []) known.add(t.id);
  log('baseline known threads:', known.size);
  // Subscription is now driven by the global thread/started push (see message handler).
});

async function subscribeWithRetry(threadId, tries) {
  for (let i = 0; i < tries; i++) {
    try { await call('thread/resume', { threadId }); log('  → SUBSCRIBED to', String(threadId).slice(-8), `(try ${i + 1})`); return; }
    catch (e) { if (i === tries - 1) log('  subscribe gave up:', e.message); await new Promise(r => setTimeout(r, 250)); }
  }
}

async function poll() {
  try {
    const list = await call('thread/list', { limit: 20 });
    for (const t of list.data || []) {
      if (!known.has(t.id)) {
        known.add(t.id);
        log('NEW THREAD detected via poll:', t.id.slice(-12), '|', (t.preview || t.name || '').slice(0, 40));
        try { await call('thread/resume', { threadId: t.id }); log('  → resumed (subscribed) to', t.id.slice(-12)); }
        catch (e) { log('  resume failed:', e.message); }
      }
    }
  } catch {}
}

ws.on('message', (buf) => {
  let m; try { m = JSON.parse(buf.toString()); } catch { return; }
  if (m.id !== undefined && P.has(m.id)) { const x = P.get(m.id); P.delete(m.id); return m.error ? x.rej(new Error(JSON.stringify(m.error))) : x.res(m.result); }
  if (m.method) {
    const skip = ['item/agentMessage/delta', 'item/reasoning/summaryTextDelta', 'item/reasoning/textDelta', 'app/list/updated'];
    if (skip.includes(m.method)) return;
    const p = m.params || {};
    const threadId = p.threadId || p.thread?.id;
    const tag = threadId ? 'thread=' + String(threadId).slice(-8) : '';
    const turn = p.turn?.id ? 'turn=' + String(p.turn.id).slice(-8) : '';
    log('PUSH', m.method, tag, turn);
    // Real-time subscribe: on a global thread/started, rejoin so we get its turn events.
    // The rollout file may not be on disk the instant thread/started fires, so retry briefly.
    if (AUTO && m.method === 'thread/started' && threadId && !known.has(threadId)) {
      known.add(threadId);
      subscribeWithRetry(threadId, 8);
    }
  }
});
ws.on('error', (e) => { log('ERR', e.message); process.exit(1); });
ws.on('close', () => log('closed'));
setTimeout(() => { log('done'); process.exit(0); }, 90000);
