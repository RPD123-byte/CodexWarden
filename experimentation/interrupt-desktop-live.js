// Poll a desktop thread until a turn is inProgress, then try turn/interrupt from OUR process.
// Usage: node interrupt-desktop-live.js <threadId> [timeoutSecs]
const { CodexController, DESKTOP_BINARY } = require('./codex-controller');

const threadId = process.argv[2];
const timeoutSecs = Number(process.argv[3] ?? 120);
const t0 = Date.now();
const log = (tag, ...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(2)}s [${tag}]`, ...a);

(async () => {
  const ctl = new CodexController({ binary: DESKTOP_BINARY });
  ctl.on('notification', (m) => {
    if (m.method.startsWith('turn/') || m.method.startsWith('thread/status')) {
      log('event', m.method, JSON.stringify(m.params ?? {}).slice(0, 150));
    }
  });
  ctl.start();
  await ctl.initialize('desktop-interruptor');
  await ctl.request('thread/resume', { threadId }).then(
    () => log('resumed', threadId),
    (e) => log('resume-failed', e.message)
  );

  const deadline = t0 + timeoutSecs * 1000;
  while (Date.now() < deadline) {
    let th;
    try {
      const r = await ctl.request('thread/read', { threadId, includeTurns: true });
      th = r?.thread ?? r;
    } catch (e) { log('read-failed', e.message); await sleep(1000); continue; }
    const active = (th?.turns ?? []).filter((t) => t.status === 'inProgress');
    if (active.length) {
      const turnId = active[active.length - 1].id;
      log('ACTIVE-TURN', turnId, 'threadStatus=', JSON.stringify(th.status));
      try {
        const r = await ctl.request('turn/interrupt', { threadId, turnId });
        log('interrupt-response', JSON.stringify(r));
      } catch (e) {
        log('interrupt-failed', e.message);
      }
      // Check aftermath for a few seconds
      for (let i = 0; i < 6; i++) {
        await sleep(2000);
        const r2 = await ctl.request('thread/read', { threadId, includeTurns: true }).catch(() => null);
        const t2 = (r2?.thread?.turns ?? []).find((t) => t.id === turnId);
        log('aftermath', `turn=${t2?.status}`, 'threadStatus=', JSON.stringify(r2?.thread?.status));
      }
      ctl.stop(); process.exit(0);
    }
    log('poll', 'idle; turns=', (th?.turns ?? []).length);
    await sleep(1500);
  }
  log('timeout', 'no active turn observed');
  ctl.stop(); process.exit(1);
})();

function sleep(ms) { return new Promise((r) => setTimeout(r, ms)); }
