// Acoustic loopback on one machine: two Chrome windows run the app (dev server), one receives and
// one sends, through the real default speaker and microphone.
//
//   pnpm dev &                      # http://localhost:5199
//   node scripts/live-loopback.mjs [adaptive|classic] [message] [timeout_s]
//
// Chrome is told to auto-accept the page's microphone prompt (macOS may still ask once).
import { chromium } from 'playwright';

const mode = process.argv[2] ?? 'adaptive';
const message = process.argv[3] ?? 'Hello through the real room: speaker, air, microphone.';
const timeoutS = Number(process.argv[4] ?? (mode === 'classic' ? 180 : 150));
const url = process.env.URL ?? 'http://localhost:5199/';
const band = { lo: Number(process.env.LO ?? 800), hi: Number(process.env.HI ?? 10000) };

const browser = await chromium.launch({
  channel: 'chrome',
  headless: false,
  args: [
    '--use-fake-ui-for-media-stream',
    '--autoplay-policy=no-user-gesture-required',
    '--disable-background-timer-throttling',
    '--disable-renderer-backgrounding',
    '--disable-backgrounding-occluded-windows',
  ],
});

async function open(name, x) {
  const ctx = await browser.newContext({ permissions: ['microphone'], viewport: { width: 400, height: 820 } });
  const page = await ctx.newPage();
  page.on('pageerror', (e) => console.log(`[${name}] page error: ${e.message}`));
  page.on('console', (m) => m.type() === 'error' && console.log(`[${name}] console: ${m.text()}`));
  await page.goto(url);
  await page.evaluate(
    ([mode, band]) => localStorage.setItem('dos.settings.v1', JSON.stringify({ simulate: false, mode, ...band, volume: 1 })),
    [mode, band],
  );
  await page.reload();
  await page.waitForFunction(() => window.__dos?.app.ready, null, { timeout: 20000 });
  void x;
  return page;
}

const snap = (page) =>
  page.evaluate(() => {
    const { app } = window.__dos;
    const s = app.status;
    const c = app.classic;
    return {
      error: app.error,
      level: app.level,
      activity: app.activity,
      received: app.received.map((r) => r.message ?? `${r.name} (${r.size} B)`),
      ofdm: s && { role: s.role, state: s.state, sent: s.frames_sent, heard: s.frames_heard, missed: s.frames_missed, fa: s.false_alarms, undelivered: s.send_undelivered, total: s.send_total, rx: s.recv_progress, log: s.log.at(-1) },
      classic: c && { state: c.state, packet: c.packet, total: c.total, retries: c.retries, heard: c.frames_heard, log: c.log.at(-1) },
    };
  });

const rx = await open('rx');
const tx = await open('tx');
await rx.click('nav.tabs-mobile >> text=Receive');
await rx.click('role=button[name="Start listening"]');
await tx.waitForTimeout(1500);
await tx.fill('#msg', message);
await tx.click('text=Send message');
console.log(`mode=${mode} band=${band.lo}-${band.hi} Hz, sending ${JSON.stringify(message)}`);

const t0 = Date.now();
let last = '';
let result = null;
while ((Date.now() - t0) / 1000 < timeoutS) {
  await tx.waitForTimeout(1000);
  const [a, b] = await Promise.all([snap(tx), snap(rx)]);
  const line = JSON.stringify({ tx: a.ofdm ?? a.classic, rx: (b.ofdm?.log ?? '') + ' | ' + (b.classic?.log ?? ''), rxLevel: b.level.toFixed(4), err: a.error || b.error || undefined });
  if (line !== last) {
    console.log(`${((Date.now() - t0) / 1000).toFixed(0).padStart(4)}s ${line}`);
    last = line;
  }
  const txDone = a.ofdm?.state === 'done' || a.classic?.state === 'done' || a.classic?.state === 'failed';
  if (b.received.length && txDone) {
    result = b.received;
    break;
  }
}
const [a, b] = await Promise.all([snap(tx), snap(rx)]);
console.log('received:', JSON.stringify(b.received));
const ok = b.received.includes(message);
console.log(ok ? `PASS in ${((Date.now() - t0) / 1000).toFixed(0)} s` : `FAIL (tx ${JSON.stringify(a.ofdm ?? a.classic)})`);
await browser.close();
process.exit(ok ? 0 : 1);
