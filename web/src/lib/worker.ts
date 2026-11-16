/// <reference lib="webworker" />
// Runs the dos modems off the main thread.
// - live: microphone chunks arrive on a MessagePort from the capture worklet; waveforms to transmit
//   are posted to the main thread to play.
// - simulated: a second device shares the measured room (data/channel_*.json) with this one, and
//   the air is advanced on a timer, so every screen works without audio.
//
// A device holds one or two "radios" that share its microphone and take turns on its speaker:
// the adaptive OFDM station and the classic (dos modem) station. Listening runs both, so either
// kind of sender is heard.

import init, { Air, ClassicStation, Spectrum, Station, band_info, scenario_names } from './dos/dos.js';
import wasmUrl from './dos/dos_bg.wasm?url';
import abUrl from '../../../data/channel_dell-g5_to_e540.json?url';
import baUrl from '../../../data/channel_e540_to_dell-g5.json?url';
import type { Band, BandInfo, ClassicStatus, FromWorker, Mode, SimSetup, StationStatus, ToWorker, View } from './types';
import { FLAG_MESSAGE } from './types';

const ctx = self as unknown as DedicatedWorkerGlobalScope;
const BLOCK = 960;
const BLOCK_MS = (BLOCK / 48_000) * 1000;
const seed = () => BigInt(Math.floor(Math.random() * 2 ** 31));

interface Radio {
  kind: Mode;
  push(samples: Float32Array, now: number): void;
  poll(now: number): Float32Array | undefined;
  txDone(now: number): void;
  free(): void;
}

function ofdmRadio(st: Station): Radio & { st: Station } {
  return {
    kind: 'adaptive',
    st,
    push: (s) => st.push_audio(s),
    poll: (now) => st.poll(now),
    txDone: (now) => st.tx_done(now),
    free: () => st.free(),
  };
}

function classicRadio(st: ClassicStation): Radio & { st: ClassicStation } {
  return {
    kind: 'classic',
    st,
    push: (s, now) => st.push_audio(s, now),
    poll: (now) => st.poll(now),
    txDone: (now) => st.tx_done(now),
    free: () => st.free(),
  };
}

/** One device: its radios and which of them currently owns the speaker. */
class Device {
  ofdm: (Radio & { st: Station }) | null = null;
  classic: (Radio & { st: ClassicStation }) | null = null;
  owner: Radio | null = null;
  left = 0;

  radios(): Radio[] {
    return [this.ofdm, this.classic].filter((r): r is NonNullable<typeof r> => r !== null);
  }

  hear(samples: Float32Array, now: number) {
    for (const r of this.radios()) r.push(samples, now);
  }

  /** A waveform to start playing, if this device is silent and a radio wants the air. */
  next(now: number): Float32Array | null {
    if (this.owner) return null;
    for (const r of this.radios()) {
      const w = r.poll(now);
      if (w) {
        this.owner = r;
        this.left = w.length;
        return w;
      }
    }
    return null;
  }

  ended(now: number) {
    this.owner?.txDone(now);
    this.owner = null;
    this.left = 0;
  }

  clear() {
    for (const r of this.radios()) r.free();
    this.ofdm = this.classic = this.owner = null;
    this.left = 0;
  }
}

let band: Band = { lo: 800, hi: 10_000 };
let info: BandInfo | null = null;
let simulate = true;
let sim: SimSetup = { speed: 1, scenario: 0 };
let view: View = { lo: 0, hi: 12_000, bins: 160 };
let spectrum: Spectrum | null = null;
const local = new Device();
const peer = new Device();
let air: Air | null = null;
let channels: [string, string] | null = null;
let timer: ReturnType<typeof setInterval> | null = null;
let clock = 0;
let rows: Float32Array[] = [];
let level = 0;
let micPort: MessagePort | null = null;

let listening = false;
const done = { ofdm: null as number | null, classic: null as number | null };
let lastHeard = { frames: 0, at: 0 };
let demoIndex = 0;
let demoNextAt: number | null = null;
let idlePosted = false;
let activityClock = 0;

function post(msg: FromWorker, transfer: Transferable[] = []) {
  ctx.postMessage(msg, transfer);
}

function fail(e: unknown) {
  post({ t: 'error', message: e instanceof Error ? e.message : String(e) });
}

function now(): number {
  return simulate ? clock : performance.now();
}

const ofdmStatus = (d: Device): StationStatus | null => (d.ofdm ? JSON.parse(d.ofdm.st.status_json()) : null);
const classicStatus = (d: Device): ClassicStatus | null => (d.classic ? JSON.parse(d.classic.st.status_json()) : null);

async function loadChannels(): Promise<[string, string]> {
  channels ??= await Promise.all([fetch(abUrl).then((r) => r.text()), fetch(baUrl).then((r) => r.text())]);
  return channels;
}

function reset() {
  local.clear();
  peer.clear();
  listening = false;
  done.ofdm = done.classic = null;
  demoNextAt = null;
  idlePosted = false;
}

async function configure(msg: Extract<ToWorker, { t: 'config' }>) {
  if (timer) clearInterval(timer);
  timer = null;
  reset();
  spectrum?.free();
  air?.free();
  air = null;
  band = msg.band;
  simulate = msg.simulate;
  sim = msg.sim;
  view = msg.view;
  info = JSON.parse(band_info(band.lo, band.hi));
  spectrum = new Spectrum(view.lo, view.hi, view.bins);
  if (simulate) {
    const [ab, ba] = await loadChannels();
    air = new Air(ab, ba, seed());
    air.set_scenario(sim.scenario);
    timer = setInterval(simTick, BLOCK_MS);
  } else {
    timer = setInterval(livePump, 100);
  }
  post({ t: 'ready', info: info!, scenarios: JSON.parse(scenario_names()) });
}

function hearLocal(samples: Float32Array, at: number) {
  if (spectrum) {
    const r = spectrum.push(samples);
    if (r.length) rows.push(r);
  }
  let e = 0;
  for (let i = 0; i < samples.length; i++) e += samples[i] * samples[i];
  level = level * 0.85 + Math.sqrt(e / samples.length) * 0.15;
  local.hear(samples, at);
}

/** Classic transfers carry raw bytes (as `dos modem` does): show text as a message. */
function classify(data: Uint8Array): { name: string; flags: number } {
  try {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(data);
    if (data.length <= 4096 && !/[\u0000-\u0008\u000e-\u001f]/.test(text)) return { name: '', flags: FLAG_MESSAGE };
  } catch {
    /* binary */
  }
  return { name: 'received.bin', flags: 0 };
}

function flush() {
  if (rows.length) {
    const total = rows.reduce((n, r) => n + r.length, 0);
    const data = new Float32Array(total);
    let o = 0;
    for (const r of rows) {
      data.set(r, o);
      o += r.length;
    }
    rows = [];
    post({ t: 'rows', data, bins: view.bins }, [data.buffer]);
  }
  for (;;) {
    const c = local.ofdm?.st.take_completed();
    if (!c) break;
    const data = new Uint8Array(c.data);
    post({ t: 'completed', file: { name: c.name, mime: c.mime, flags: c.flags, data: data.buffer } }, [data.buffer]);
    c.free();
    done.ofdm = now();
  }
  const raw = local.classic?.st.take_completed();
  if (raw) {
    const data = new Uint8Array(raw);
    const { name, flags } = classify(data);
    post({ t: 'completed', file: { name, mime: flags ? 'text/plain' : 'application/octet-stream', flags, data: data.buffer } }, [data.buffer]);
    done.classic = now();
  }
  for (;;) {
    const c = peer.ofdm?.st.take_completed();
    if (!c) break;
    c.free();
  }
  peer.classic?.st.take_completed();
  maybeStatus();
}

let lastStatus = 0;
function maybeStatus(force = false) {
  const t = performance.now();
  if (!force && t - lastStatus < 250) return;
  lastStatus = t;
  post({ t: 'status', status: ofdmStatus(local), classic: classicStatus(local), level, air_s: (clock - activityClock) / 1000 });
}

/** Receivers handle one incoming stream each; after one completes and the air goes quiet, start afresh. */
function receiverHousekeeping() {
  if (!listening || !info) return;
  const o = ofdmStatus(local);
  const c = classicStatus(local);
  const heard = (o?.frames_heard ?? 0) + (c?.frames_heard ?? 0);
  if (heard !== lastHeard.frames) lastHeard = { frames: heard, at: now() };
  const quiet = now() - lastHeard.at > (info.frame_s * 2 + 8) * 1000;
  if (!quiet || local.owner) return;
  if (done.ofdm !== null && local.ofdm) {
    local.ofdm.free();
    local.ofdm = ofdmRadio(new Station(band.lo, band.hi, 'follower', seed()));
    done.ofdm = null;
  }
  if (done.classic !== null && local.classic) {
    local.classic.free();
    local.classic = classicRadio(ClassicStation.receiver());
    done.classic = null;
  }
  lastHeard = { frames: 0, at: now() };
}

function senderHousekeeping() {
  if (listening) return;
  const o = ofdmStatus(local);
  const c = classicStatus(local);
  const finished = (o?.role === 'leader' && o.state === 'done') || (c?.sending && (c.state === 'done' || c.state === 'failed'));
  if (finished && !idlePosted) post({ t: 'idle' });
  idlePosted = !!finished;
}

const DEMOS: { mode: Mode; name: string; mime: string; message: boolean; text: string }[] = [
  { mode: 'adaptive', name: '', mime: 'text/plain', message: true, text: 'Meet at the north entrance at 7. The wifi here is down, so this came by sound.' },
  {
    mode: 'adaptive',
    name: 'field-notes.txt',
    mime: 'text/plain',
    message: false,
    text: Array.from({ length: 24 }, (_, i) => `Reading ${i + 1}: tile ${i * 4}, ${(800 + i * 380).toLocaleString()} Hz, SNR ${(18 - i * 0.4).toFixed(1)} dB`).join('\n'),
  },
  { mode: 'classic', name: '', mime: 'text/plain', message: true, text: 'Hello from the classic 100 baud modem.' },
];

function startDemo() {
  const d = DEMOS[demoIndex++ % DEMOS.length];
  peer.clear();
  const bytes = new TextEncoder().encode(d.text);
  if (d.mode === 'adaptive') {
    peer.ofdm = ofdmRadio(Station.sending(band.lo, band.hi, 'leader', seed(), d.name, d.mime, d.message ? FLAG_MESSAGE : 0, bytes));
  } else {
    peer.classic = classicRadio(ClassicStation.sender(bytes));
  }
  demoNextAt = null;
}

function demoHousekeeping() {
  if (!listening) return;
  const o = ofdmStatus(peer);
  const c = classicStatus(peer);
  const finished = (o && o.state === 'done') || (c && (c.state === 'done' || c.state === 'failed'));
  if (!finished) return;
  demoNextAt ??= clock + 10_000;
  if (clock >= demoNextAt) startDemo();
}

function simTick() {
  if (!air) return;
  try {
    for (let step = 0; step < sim.speed; step++) {
      const devices = [local, peer];
      for (let i = 0; i < 2; i++) {
        const heard = air.hear(i, BLOCK);
        if (i === 0) hearLocal(heard, clock);
        else peer.hear(heard, clock);
        const w = devices[i].next(clock);
        if (w) air.transmit(i, w);
      }
      clock += BLOCK_MS;
      for (const d of devices) {
        if (d.owner) {
          d.left -= BLOCK;
          if (d.left <= 0) d.ended(clock);
        }
      }
    }
    receiverHousekeeping();
    demoHousekeeping();
    senderHousekeeping();
    flush();
  } catch (e) {
    if (timer) clearInterval(timer);
    timer = null;
    fail(e);
  }
}

function livePump() {
  try {
    const w = local.next(performance.now());
    if (w) post({ t: 'play', samples: w }, [w.buffer]);
    receiverHousekeeping();
    senderHousekeeping();
    flush();
  } catch (e) {
    fail(e);
  }
}

function onMic(chunk: Float32Array) {
  if (simulate) return;
  try {
    hearLocal(chunk, performance.now());
  } catch (e) {
    fail(e);
  }
}

ctx.onmessage = async (e: MessageEvent<ToWorker>) => {
  const msg = e.data;
  try {
    await ready;
    switch (msg.t) {
      case 'config':
        await configure(msg);
        break;
      case 'send': {
        reset();
        activityClock = clock;
        const bytes = new Uint8Array(msg.data);
        if (msg.mode === 'classic') {
          local.classic = classicRadio(ClassicStation.sender(bytes));
          if (simulate) peer.classic = classicRadio(ClassicStation.receiver());
        } else {
          local.ofdm = ofdmRadio(Station.sending(band.lo, band.hi, 'leader', seed(), msg.name, msg.mime, msg.flags, bytes));
          if (simulate) peer.ofdm = ofdmRadio(new Station(band.lo, band.hi, 'follower', seed()));
        }
        maybeStatus(true);
        break;
      }
      case 'listen':
        reset();
        activityClock = clock;
        listening = true;
        lastHeard = { frames: 0, at: now() };
        local.ofdm = ofdmRadio(new Station(band.lo, band.hi, 'follower', seed()));
        local.classic = classicRadio(ClassicStation.receiver());
        if (simulate) {
          demoIndex = 0;
          startDemo();
        }
        maybeStatus(true);
        break;
      case 'stop':
        reset();
        maybeStatus(true);
        break;
      case 'scenario':
        sim.scenario = msg.index;
        air?.set_scenario(msg.index);
        break;
      case 'mic_port':
        micPort = msg.port;
        micPort.onmessage = (m: MessageEvent<Float32Array>) => onMic(m.data);
        break;
      case 'played':
        local.ended(performance.now());
        break;
    }
  } catch (err) {
    fail(err);
  }
};

const ready = init({ module_or_path: wasmUrl });
