// Attach to a Codex Desktop-created thread from our own app-server.
// Usage: node attach-desktop.js <threadId> [--interrupt] [--watch secs]
// Resumes the thread, dumps its status/turns, optionally watches for live
// events and tries turn/interrupt on any active turn it observes.

const { CodexController } = require('./codex-controller');

const threadId = process.argv[2];
const doInterrupt = process.argv.includes('--interrupt');
const wIdx = process.argv.indexOf('--watch');
const watchSecs = wIdx > -1 ? Number(process.argv[wIdx + 1]) : 0;
if (!threadId) { console.error('usage: node attach-desktop.js <threadId> [--interrupt] [--watch secs]'); process.exit(1); }

const t0 = Date.now();
const log = (tag, ...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(2)}s [${tag}]`, ...a);

(async () => {
  const ctl = new CodexController({ binary: 'codex' });
  ctl.on('notification', (m) => {
    const skip = ['item/agentMessage/delta', 'item/reasoning/summaryTextDelta', 'item/reasoning/textDelta', 'mcpServer/startupStatus/updated'];
    if (!skip.includes(m.method)) log('event', m.method, JSON.stringify(m.params ?? {}).slice(0, 180));
  });
  ctl.on('serverRequest', (m) => log('serverRequest', m.method));
  ctl.start();
  await ctl.initialize('desktop-attacher');

  try {
    const resumed = await ctl.request('thread/resume', { threadId });
    log('resume-ok', JSON.stringify(resumed).slice(0, 400));
  } catch (e) {
    log('resume-failed', e.message);
  }

  try {
    const read = await ctl.request('thread/read', { threadId, includeTurns: true });
    const th = read?.thread ?? read;
    log('read', 'status=', JSON.stringify(th?.status), 'turns=', (th?.turns ?? []).length);
    const active = (th?.turns ?? []).filter((t) => t.status === 'inProgress');
    log('active-turns', JSON.stringify(active.map((t) => t.id)));
    if (doInterrupt && active[0]) {
      log('INTERRUPT', active[0].id);
      try {
        const r = await ctl.interrupt(threadId, active[0].id);
        log('interrupt-ok', JSON.stringify(r));
      } catch (e) {
        log('interrupt-failed', e.message);
      }
    }
  } catch (e) {
    log('read-failed', e.message);
  }

  if (watchSecs > 0) {
    log('watching', `${watchSecs}s for live events on this thread...`);
    if (doInterrupt) {
      ctl.on('turn/started', async (p) => {
        const turnId = p?.turn?.id;
        log('live-turn-started', turnId, '— interrupting');
        try { log('interrupt-ok', JSON.stringify(await ctl.interrupt(threadId, turnId))); }
        catch (e) { log('interrupt-failed', e.message); }
      });
    }
    setTimeout(() => { ctl.stop(); process.exit(0); }, watchSecs * 1000);
  } else {
    ctl.stop();
    process.exit(0);
  }
})();
