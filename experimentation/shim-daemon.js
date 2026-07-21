// Stand-in for the facial-expression daemon. Connects to the shim's side socket,
// watches the real app's traffic, and interrupts the active turn N seconds after
// it starts (proving out-of-band interruption of a REAL desktop-app session).
const net = require('net');
const fs = require('fs');
const readline = require('readline');
const path = require('path');

const REGISTRY = path.join(__dirname, 'shim-registry.ndjson');
const AFTER_MS = Number(process.argv[2] ?? 6000);
const t0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(2)}s`, ...a);

let armed = false;
const conns = new Map(); // sock path -> socket

function pidAlive(pid) { try { process.kill(pid, 0); return true; } catch { return false; } }

function connectTo(entry) {
  if (conns.has(entry.sock)) return;
  if (!pidAlive(entry.pid)) return;
  const c = net.connect(entry.sock, () => log('watching', path.basename(entry.sock), 'argv=', entry.argv.join(' ')));
  c.on('error', () => { conns.delete(entry.sock); });
  c.on('close', () => { conns.delete(entry.sock); });
  conns.set(entry.sock, c);
  const rl = readline.createInterface({ input: c });
  rl.on('line', (line) => onMsg(c, line));
}

function onMsg(c, line) {
  let m; try { m = JSON.parse(line); } catch { return; }
  if (m.type === 'injected') { log('INJECTED', JSON.stringify(m.frame.params)); return; }
  if (m.type === 'error') { log('inject-error', m.error); return; }
  if (m.type !== 'traffic') return;
  let msg; try { msg = JSON.parse(m.line); } catch { return; }
  if (msg.error !== undefined) { log('engine-error resp id=', msg.id, JSON.stringify(msg.error)); return; }
  if (msg.method === 'turn/started') {
    const threadId = msg.params?.threadId;
    const turnId = msg.params?.turn?.id;
    log('turn/started thread=', threadId, 'turn=', turnId, `→ interrupt in ${AFTER_MS}ms`);
    // Interrupt THIS exact pair (from its own turn/started event) after the "threshold" delay.
    setTimeout(() => {
      log('interrupting', turnId);
      c.write(JSON.stringify({ type: 'interrupt', threadId, turnId }) + '\n');
    }, AFTER_MS);
  }
  if (msg.method === 'turn/completed') {
    const status = msg.params?.turn?.status ?? msg.params?.status;
    const turnId = msg.params?.turn?.id ?? msg.params?.turnId;
    log('turn/completed turn=', turnId, 'status=', status);
    if (armed && status === 'interrupted') { log('REAL DESKTOP TURN INTERRUPTED ✅'); process.exit(0); }
  }
}

function scan() {
  let lines = [];
  try { lines = fs.readFileSync(REGISTRY, 'utf8').trim().split('\n'); } catch { return; }
  for (const l of lines) { let e; try { e = JSON.parse(l); } catch { continue; } connectTo(e); }
}

log('discovering shims via registry...');
scan();
setInterval(scan, 1000);
setTimeout(() => { log('timeout'); process.exit(1); }, 180000);
