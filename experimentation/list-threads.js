// List threads visible to our own app-server — do desktop-app threads show up?
const { CodexController } = require('./codex-controller');

(async () => {
  const ctl = new CodexController({ binary: 'codex' });
  ctl.start();
  await ctl.initialize('thread-lister');
  const res = await ctl.request('thread/list', { limit: 25 });
  const threads = res?.threads ?? res?.items ?? res;
  for (const t of Array.isArray(threads) ? threads : []) {
    console.log(
      (t.updatedAt ?? t.createdAt ?? '?'),
      t.id,
      '|', (t.preview ?? t.name ?? '').slice(0, 80).replace(/\n/g, ' ')
    );
  }
  if (!Array.isArray(threads)) console.log(JSON.stringify(res).slice(0, 2000));
  ctl.stop();
  process.exit(0);
})();
