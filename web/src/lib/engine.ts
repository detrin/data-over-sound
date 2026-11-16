// Main thread: owns the worker, the AudioContext, microphone capture and playback, and turns
// worker messages into app state.

import init, { band_info, estimate as wasmEstimate } from './dos/dos.js';
import wasmUrl from './dos/dos_bg.wasm?url';
import captureUrl from './capture-worklet.js?url';
import { app, saveSettings, settings, viewRange } from './state.svelte';
import type { BandInfo, CompletedMsg, Estimate, FromWorker, ToWorker } from './types';
import { FLAG_GZIP, FLAG_MESSAGE, SAMPLE_RATE } from './types';

type RowsListener = (rows: Float32Array, bins: number) => void;

const worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
const rowsListeners = new Set<RowsListener>();
const wasmReady = init({ module_or_path: wasmUrl });

let audio: AudioContext | null = null;
let output: GainNode | null = null;
let micStream: MediaStream | null = null;
let wakeLock: { release(): Promise<void> } | null = null;

export const VIEW_BINS = 180;

function postW(msg: ToWorker, transfer: Transferable[] = []) {
  worker.postMessage(msg, transfer);
}

export function onRows(fn: RowsListener): () => void {
  rowsListeners.add(fn);
  return () => rowsListeners.delete(fn);
}

worker.onmessage = (e: MessageEvent<FromWorker>) => {
  const m = e.data;
  switch (m.t) {
    case 'ready':
      app.ready = true;
      app.info = m.info;
      app.scenarios = m.scenarios;
      app.error = '';
      break;
    case 'error':
      app.error = m.message;
      break;
    case 'rows':
      for (const fn of rowsListeners) fn(m.data, m.bins);
      break;
    case 'status':
      app.status = m.status;
      app.classic = m.classic;
      app.level = m.level;
      app.airSeconds = m.air_s;
      break;
    case 'idle':
      app.finished = true;
      releaseWake();
      break;
    case 'play':
      play(m.samples);
      break;
    case 'completed':
      void onCompleted(m.file);
      break;
  }
};

/** Push settings to the worker; rebuilds the modem for the band. */
export function configure() {
  saveSettings();
  app.ready = false;
  app.activity = 'idle';
  app.status = null;
  app.classic = null;
  postW({
    t: 'config',
    band: { lo: settings.lo, hi: settings.hi },
    simulate: settings.simulate,
    sim: { speed: settings.simSpeed, scenario: settings.simScenario },
    view: { ...viewRange(), bins: VIEW_BINS },
  });
  if (settings.simulate) stopMic();
}

export function setScenario(index: number) {
  settings.simScenario = index;
  saveSettings();
  postW({ t: 'scenario', index });
}

export async function bandInfo(lo: number, hi: number): Promise<BandInfo | string> {
  await wasmReady;
  try {
    return JSON.parse(band_info(lo, hi));
  } catch (e) {
    return e instanceof Error ? e.message : String(e);
  }
}

export async function estimate(bytes: number): Promise<Estimate | null> {
  await wasmReady;
  try {
    return JSON.parse(wasmEstimate(settings.lo, settings.hi, bytes));
  } catch {
    return null;
  }
}

/** Live mode only: open the mic with every voice-processing stage disabled. */
export async function ensureAudio(): Promise<boolean> {
  if (settings.simulate) return true;
  try {
    audio ??= new AudioContext({ sampleRate: SAMPLE_RATE, latencyHint: 'interactive' });
    if (audio.state === 'suspended') await audio.resume();
    if (!output) {
      output = audio.createGain();
      output.connect(audio.destination);
    }
    output.gain.value = settings.volume;
    if (micStream) return true;
    micStream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 1, sampleRate: SAMPLE_RATE },
    });
    await audio.audioWorklet.addModule(captureUrl);
    const src = audio.createMediaStreamSource(micStream);
    const node = new AudioWorkletNode(audio, 'dos-capture', { numberOfInputs: 1, numberOfOutputs: 0 });
    const ch = new MessageChannel();
    node.port.postMessage({ port: ch.port2 }, [ch.port2]);
    postW({ t: 'mic_port', port: ch.port1 }, [ch.port1]);
    src.connect(node);
    return true;
  } catch (e) {
    app.error =
      e instanceof Error && e.name === 'NotAllowedError'
        ? 'Microphone access is blocked. Allow it in the browser’s site settings, then try again.'
        : `Could not open the microphone: ${e instanceof Error ? e.message : e}`;
    return false;
  }
}

function stopMic() {
  micStream?.getTracks().forEach((t) => t.stop());
  micStream = null;
}

function play(samples: Float32Array) {
  if (!audio || !output) {
    postW({ t: 'played' });
    return;
  }
  const buf = audio.createBuffer(1, samples.length, SAMPLE_RATE);
  buf.copyToChannel(samples as Float32Array<ArrayBuffer>, 0);
  const src = audio.createBufferSource();
  src.buffer = buf;
  src.connect(output);
  src.onended = () => postW({ t: 'played' });
  src.start();
}

async function holdWake() {
  try {
    const nav = navigator as Navigator & { wakeLock?: { request(t: 'screen'): Promise<{ release(): Promise<void> }> } };
    wakeLock = (await nav.wakeLock?.request('screen')) ?? null;
  } catch {
    wakeLock = null;
  }
}

function releaseWake() {
  void wakeLock?.release();
  wakeLock = null;
}

async function pipe(data: Uint8Array, t: CompressionStream | DecompressionStream): Promise<Uint8Array> {
  const stream = new Blob([data as Uint8Array<ArrayBuffer>]).stream().pipeThrough(t as unknown as TransformStream<Uint8Array, Uint8Array>);
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** Compress when it helps; returns the bytes to send and their flags. */
export async function prepare(raw: Uint8Array, message: boolean): Promise<{ data: Uint8Array; flags: number }> {
  let flags = message ? FLAG_MESSAGE : 0;
  let data = raw;
  try {
    const z = await pipe(raw, new CompressionStream('gzip'));
    if (z.length < raw.length * 0.9) {
      data = z;
      flags |= FLAG_GZIP;
    }
  } catch {
    /* no CompressionStream: send as is */
  }
  return { data, flags };
}

export async function send(raw: Uint8Array, opts: { message: boolean; name: string; mime: string; label: string }) {
  if (!(await ensureAudio())) return;
  const mode = settings.mode;
  // classic carries raw bytes like `dos modem`, so the other end gets the file exactly
  const { data, flags } = mode === 'classic' ? { data: raw, flags: opts.message ? FLAG_MESSAGE : 0 } : await prepare(raw, opts.message);
  app.activity = 'sending';
  app.finished = false;
  app.activityStarted = performance.now();
  app.sentLabel = opts.label;
  void holdWake();
  const buf = data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) as ArrayBuffer;
  postW({ t: 'send', mode, name: opts.name, mime: opts.mime, flags, data: buf }, [buf]);
}

export async function listen() {
  if (!(await ensureAudio())) return;
  app.activity = 'listening';
  app.finished = false;
  app.activityStarted = performance.now();
  void holdWake();
  postW({ t: 'listen' });
}

export function stop() {
  postW({ t: 'stop' });
  app.activity = 'idle';
  releaseWake();
}

async function onCompleted(f: CompletedMsg) {
  let data: Uint8Array = new Uint8Array(f.data);
  if (f.flags & FLAG_GZIP) {
    try {
      data = await pipe(data, new DecompressionStream('gzip'));
    } catch {
      app.error = 'A transfer arrived but could not be decompressed.';
      return;
    }
  }
  const message = f.flags & FLAG_MESSAGE ? new TextDecoder().decode(data) : null;
  const url = message === null ? URL.createObjectURL(new Blob([data as Uint8Array<ArrayBuffer>], { type: f.mime || 'application/octet-stream' })) : null;
  app.received.unshift({ id: `${Date.now()}-${Math.random()}`, at: new Date(), message, name: f.name, mime: f.mime, size: data.length, url });
  navigator.vibrate?.(60);
}
