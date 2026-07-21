// WebSocket client to the managed app-server daemon over its unix socket.
// The daemon serves the app-server JSON-RPC at ws://.../rpc on the control socket.
// This connects as a SECOND client — alongside the ChatGPT GUI — so we can
// observe and interrupt the GUI's turns when the GUI is run with
// CODEX_APP_SERVER_USE_LOCAL_DAEMON=1.
const WebSocket = require('ws');
const os = require('os'), path = require('path');

const SOCK = path.join(os.homedir(), '.codex/app-server-control/app-server-control.sock');
const URL = `ws+unix://${SOCK}:/rpc`;
const t0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(2)}s`, ...a);

const ws = new WebSocket(URL, { perMessageDeflate: false });
let nextId = 1;
const pending = new Map();
const activeTurns = new Map(); // threadId -> turnId

const send = (method, params = {}) => {
  const id = nextId++;
  ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
  return new Promise((res, rej) => pending.set(id, { res, rej }));
};

ws.on('open', async () => {
  log('WS OPEN to daemon');
  const init = await send('initialize', { clientInfo: { name: 'daemon-second-client', title: 'lab', version: '0.1.0' } });
  ws.send(JSON.stringify({ jsonrpc: '2.0', method: 'initialized', params: {} }));
  log('initialized:', JSON.stringify(init).slice(0, 120));
  const list = await send('thread/list', { limit: 5 });
  const data = list?.data ?? list;
  log('thread/list returned', Array.isArray(data) ? data.length : '?', 'threads:');
  for (const t of (Array.isArray(data) ? data : []).slice(0, 5)) {
    console.log('   ', t.id?.slice(-12), '|', (t.preview || t.name || '').slice(0, 50).replace(/\n/g, ' '));
  }
  if (process.argv.includes('--watch')) {
    log('watching for turns (will interrupt any that start)...');
  } else {
    ws.close(); process.exit(0);
  }
});

ws.on('message', (buf) => {
  let m; try { m = JSON.parse(buf.toString()); } catch { return; }
  if (m.id !== undefined && pending.has(m.id)) {
    const p = pending.get(m.id); pending.delete(m.id);
    return m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result);
  }
  if (m.method === 'turn/started') {
    const threadId = m.params?.threadId, turnId = m.params?.turn?.id;
    activeTurns.set(threadId, turnId);
    log('SAW GUI turn/started thread=', threadId?.slice(-12), 'turn=', turnId?.slice(-12));
    if (process.argv.includes('--watch')) {
      const delay = 4000;
      log(`  → interrupting in ${delay}ms`);
      setTimeout(async () => {
        try { const r = await send('turn/interrupt', { threadId, turnId }); log('  INTERRUPT sent:', JSON.stringify(r)); }
        catch (e) { log('  interrupt error:', e.message); }
      }, delay);
    }
  }
  if (m.method === 'turn/completed') {
    const st = m.params?.turn?.status ?? m.params?.status;
    log('GUI turn/completed status=', st, m.params?.turn?.id?.slice(-12));
  }
});

ws.on('error', (e) => { log('WS ERROR', e.message); process.exit(1); });
ws.on('close', () => log('WS closed'));
