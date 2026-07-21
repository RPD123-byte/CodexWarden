#!/usr/bin/env node
// Probe ~/.codex/ipc/ipc.sock — try JSON-RPC initialize with newline framing.
const net = require('net');
const os = require('os');
const path = require('path');

const SOCK = path.join(os.homedir(), '.codex/ipc/ipc.sock');
const framing = process.argv[2] || 'ndjson'; // 'ndjson' | 'lsp'

const msg = {
  jsonrpc: '2.0',
  id: 1,
  method: 'initialize',
  params: { clientInfo: { name: 'codex-control-lab', title: 'Codex Control Lab', version: '0.1.0' } },
};

const sock = net.connect(SOCK, () => {
  console.error(`[connected, framing=${framing}]`);
  const body = JSON.stringify(msg);
  if (framing === 'lsp') {
    sock.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
  } else {
    sock.write(body + '\n');
  }
});

sock.on('data', (d) => console.log('[recv]', d.toString()));
sock.on('error', (e) => { console.error('[error]', e.message); process.exit(1); });
sock.on('close', () => { console.error('[closed]'); process.exit(0); });
setTimeout(() => { console.error('[timeout 8s]'); process.exit(2); }, 8000);
