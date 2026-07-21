// Live test: create a thread, start a long-running turn, interrupt it mid-flight.
// Usage: node demo-interrupt.js [--desktop-binary] [--delay ms]

const { CodexController, DESKTOP_BINARY } = require('./codex-controller');

const useDesktop = process.argv.includes('--desktop-binary');
const delayIdx = process.argv.indexOf('--delay');
const INTERRUPT_AFTER_MS = delayIdx > -1 ? Number(process.argv[delayIdx + 1]) : 6000;

const log = (tag, ...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(2)}s [${tag}]`, ...a);
const t0 = Date.now();

(async () => {
  const ctl = new CodexController({
    binary: useDesktop ? DESKTOP_BINARY : 'codex',
    verbose: false,
  });

  ctl.on('notification', (msg) => {
    const skip = ['item/agentMessage/delta', 'item/reasoning/summaryTextDelta', 'item/reasoning/textDelta'];
    if (!skip.includes(msg.method)) log('event', msg.method);
  });
  ctl.on('item/agentMessage/delta', (p) => process.stdout.write(p?.delta ?? ''));
  ctl.on('serverRequest', (msg) => {
    log('serverRequest', msg.method, '— auto-declining');
    ctl.respond(msg.id, { decision: 'denied' });
  });
  ctl.on('exit', ({ code }) => log('server-exit', code));

  ctl.start();
  const init = await ctl.initialize();
  log('init', JSON.stringify(init).slice(0, 160));

  const thread = await ctl.startThread({ cwd: process.cwd(), ephemeral: false });
  const threadId = thread?.thread?.id ?? thread?.threadId ?? thread?.id;
  log('thread', threadId, JSON.stringify(thread).slice(0, 200));

  // A prompt that keeps the model busy long enough to interrupt.
  const turnPromise = ctl.startTurn(
    threadId,
    'Write a very detailed 2000-word essay about the history of Unix signals. Take your time and be thorough.'
  );

  ctl.once('turn/started', async (p) => {
    const turnId = p?.turn?.id ?? p?.turnId;
    log('turn-started', 'turnId=', turnId);
    setTimeout(async () => {
      log('INTERRUPT', `sending turn/interrupt for ${threadId}/${turnId}`);
      try {
        const res = await ctl.interrupt(threadId, turnId);
        log('interrupt-ok', JSON.stringify(res));
      } catch (e) {
        log('interrupt-failed', e.message);
      }
    }, INTERRUPT_AFTER_MS);
  });

  ctl.once('turn/completed', (p) => {
    console.log();
    log('turn-completed', 'status=', JSON.stringify(p?.turn?.status ?? p).slice(0, 300));
    ctl.stop();
    process.exit(0);
  });

  turnPromise.then(
    (r) => log('turn-request-resolved', JSON.stringify(r).slice(0, 200)),
    (e) => log('turn-request-rejected', e.message)
  );

  setTimeout(() => { log('timeout', 'giving up after 120s'); ctl.stop(); process.exit(1); }, 120000);
})();
