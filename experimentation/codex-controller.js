// CodexController — owns a `codex app-server` process over stdio (ndjson JSON-RPC)
// and exposes programmatic thread/turn control: start, steer, interrupt.
//
// Uses the same binary and ~/.codex state as the ChatGPT desktop app, so threads
// created here share auth/config/session storage with the app.

const { spawn } = require('child_process');
const { EventEmitter } = require('events');
const readline = require('readline');

const DESKTOP_BINARY = '/Applications/ChatGPT.app/Contents/Resources/codex';

class CodexController extends EventEmitter {
  constructor({ binary = 'codex', verbose = false } = {}) {
    super();
    this.binary = binary;
    this.verbose = verbose;
    this.nextId = 1;
    this.pending = new Map(); // id -> {resolve, reject}
    this.activeTurns = new Map(); // threadId -> turnId
    this.proc = null;
  }

  start() {
    this.proc = spawn(this.binary, ['app-server'], { stdio: ['pipe', 'pipe', 'pipe'] });
    this.proc.stderr.on('data', (d) => this.verbose && process.stderr.write(`[server] ${d}`));
    this.proc.on('exit', (code, sig) => this.emit('exit', { code, sig }));

    const rl = readline.createInterface({ input: this.proc.stdout });
    rl.on('line', (line) => {
      let msg;
      try { msg = JSON.parse(line); } catch { return; }
      this._handle(msg);
    });
  }

  _handle(msg) {
    if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
      const p = this.pending.get(msg.id);
      if (p) {
        this.pending.delete(msg.id);
        msg.error ? p.reject(new Error(JSON.stringify(msg.error))) : p.resolve(msg.result);
      }
      return;
    }
    if (msg.method && msg.id !== undefined) {
      // Server-initiated request (approvals etc.) — surface it; caller must respond.
      this.emit('serverRequest', msg);
      return;
    }
    if (msg.method) {
      if (msg.method === 'turn/started') {
        const { threadId, turn } = msg.params || {};
        const turnId = turn?.id ?? msg.params?.turnId;
        if (threadId && turnId) this.activeTurns.set(threadId, turnId);
      }
      if (msg.method === 'turn/completed') {
        const threadId = msg.params?.threadId;
        if (threadId) this.activeTurns.delete(threadId);
      }
      this.emit('notification', msg);
      this.emit(msg.method, msg.params);
    }
  }

  request(method, params = {}) {
    const id = this.nextId++;
    const payload = JSON.stringify({ jsonrpc: '2.0', id, method, params });
    if (this.verbose) console.error(`[send] ${payload.slice(0, 200)}`);
    this.proc.stdin.write(payload + '\n');
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }

  respond(id, result) {
    this.proc.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, result }) + '\n');
  }

  async initialize(name = 'codex-control-lab') {
    return this.request('initialize', {
      clientInfo: { name, title: 'Codex Control Lab', version: '0.1.0' },
    });
  }

  async startThread(opts = {}) {
    return this.request('thread/start', opts);
  }

  async startTurn(threadId, text, opts = {}) {
    return this.request('turn/start', {
      threadId,
      input: [{ type: 'text', text }],
      ...opts,
    });
  }

  // The whole point: cancel the active turn on a thread.
  async interrupt(threadId, turnId = this.activeTurns.get(threadId)) {
    if (!turnId) throw new Error(`no active turn known for thread ${threadId}`);
    return this.request('turn/interrupt', { threadId, turnId });
  }

  // Softer intervention: inject guidance into the running turn.
  async steer(threadId, text, expectedTurnId = this.activeTurns.get(threadId)) {
    if (!expectedTurnId) throw new Error(`no active turn known for thread ${threadId}`);
    return this.request('turn/steer', {
      threadId,
      expectedTurnId,
      input: [{ type: 'text', text }],
    });
  }

  stop() {
    if (this.proc) this.proc.kill();
  }
}

module.exports = { CodexController, DESKTOP_BINARY };
