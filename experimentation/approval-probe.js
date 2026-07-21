// Does a SECOND client on the daemon receive server->client approval REQUESTS
// (exec/apply-patch approvals) that belong to the GUI's turn? We must NOT.
// Logs every message; flags server-initiated requests (method + id present).
// Deliberately never responds to them.
const WebSocket = require('ws');
const os = require('os'), path = require('path');
const SOCK = path.join(os.homedir(), '.codex/app-server-control/app-server-control.sock');
const ws = new WebSocket(`ws+unix://${SOCK}:/rpc`, { perMessageDeflate: false });
const t0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(2)}s`, ...a);
let id = 1; const P = new Map(); const subscribed = new Set();
const call = (m, p = {}) => { const i = id++; ws.send(JSON.stringify({ jsonrpc: '2.0', id: i, method: m, params: p })); return new Promise((r, j) => P.set(i, { r, j })); };
async function sub(threadId) {
  if (subscribed.has(threadId)) return; subscribed.add(threadId);
  for (let i = 0; i < 8; i++) { try { await call('thread/resume', { threadId }); log('subscribed', threadId.slice(-8)); return; } catch { await new Promise(r => setTimeout(r, 250)); } }
}
ws.on('open', async () => {
  const init = await call('initialize', { clientInfo: { name: 'approval-probe', title: 'p', version: '0.1.0' } });
  ws.send(JSON.stringify({ jsonrpc: '2.0', method: 'initialized', params: {} }));
  log('READY — will flag any server->client REQUEST we receive (and never answer it)');
  log('our declared capabilities in init response context:', JSON.stringify(init).slice(0, 120));
});
ws.on('message', (buf) => {
  let m; try { m = JSON.parse(buf.toString()); } catch { return; }
  // response to our own call
  if (m.id !== undefined && P.has(m.id)) { const x = P.get(m.id); P.delete(m.id); return m.error ? x.j(new Error(JSON.stringify(m.error))) : x.r(m.result); }
  // SERVER -> CLIENT REQUEST: has BOTH method and id, and we didn't send it
  if (m.method && m.id !== undefined) {
    log('*** SERVER REQUEST RECEIVED ***', m.method, 'id=' + m.id, JSON.stringify(m.params || {}).slice(0, 160));
    log('    (NOT responding — testing whether we even get routed these)');
    return;
  }
  if (m.method) {
    const p = m.params || {};
    if (m.method === 'thread/started' && p.thread?.id) sub(p.thread.id);
    if (/approval|Approval|exec|patch|permission/i.test(m.method) || m.method.startsWith('turn/') || m.method === 'item/started')
      log('note', m.method, (p.threadId || p.thread?.id || '').slice(-8));
  }
});
ws.on('error', (e) => { log('ERR', e.message); process.exit(1); });
setTimeout(() => { log('done'); process.exit(0); }, 120000);
