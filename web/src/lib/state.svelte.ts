import type { BandInfo, ClassicStatus, Mode, StationStatus } from './types';

export interface Settings {
  mode: Mode;
  lo: number;
  hi: number;
  volume: number;
  simulate: boolean;
  simSpeed: number;
  simScenario: number;
}

export interface Preset {
  id: string;
  name: string;
  detail: string;
  lo: number;
  hi: number;
}

export const PRESETS: Preset[] = [
  {
    id: 'dos',
    name: 'Standard',
    detail: '0.8–10 kHz, the band the dos program uses. Talks to dos on a laptop; fastest.',
    lo: 800,
    hi: 10_000,
  },
  { id: 'upper', name: 'Upper', detail: '4–10 kHz. Skips the low range where voices and fans are loudest.', lo: 4_000, hi: 10_000 },
  { id: 'high', name: 'High', detail: '12–18 kHz. Quieter to the ear; needs speakers and mics that reach it.', lo: 12_000, hi: 18_000 },
];

const KEY = 'soundlink.dos.settings.v1';

/** Simulation is a development tool: only `pnpm dev` offers it; production always uses real audio. */
export const SIM_AVAILABLE = import.meta.env.DEV;

const DEFAULTS: Settings = { mode: 'adaptive', lo: 800, hi: 10_000, volume: 1, simulate: SIM_AVAILABLE, simSpeed: 1, simScenario: 0 };

function load(): Settings {
  try {
    const s: Settings = { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? '{}') };
    if (!SIM_AVAILABLE) s.simulate = false;
    return s;
  } catch {
    return { ...DEFAULTS };
  }
}

export const settings = $state<Settings>(load());

export function saveSettings() {
  localStorage.setItem(KEY, JSON.stringify(settings));
}

export type Page = 'send' | 'receive' | 'link' | 'band';

export interface Received {
  id: string;
  at: Date;
  message: string | null;
  name: string;
  mime: string;
  size: number;
  url: string | null;
}

export const app = $state({
  page: 'send' as Page,
  ready: false,
  error: '',
  info: null as BandInfo | null,
  scenarios: [] as string[],
  status: null as StationStatus | null,
  classic: null as ClassicStatus | null,
  level: 0,
  airSeconds: 0,
  activity: 'idle' as 'idle' | 'sending' | 'listening',
  activityStarted: 0,
  finished: false,
  sentLabel: '',
  received: [] as Received[],
});

/** A transfer is running (a finished one doesn't block settings). */
export function isBusy(): boolean {
  return app.activity === 'listening' || (app.activity === 'sending' && !app.finished);
}

export function viewRange() {
  const span = settings.hi - settings.lo;
  const pad = Math.max(500, span * 0.12);
  return { lo: Math.max(0, Math.floor((settings.lo - pad) / 250) * 250), hi: Math.min(24_000, Math.ceil((settings.hi + pad) / 250) * 250) };
}

export function formatHz(hz: number): string {
  if (hz < 1000) return `${Math.round(hz)} Hz`;
  const k = hz / 1000;
  return `${Number.isInteger(k) ? k : k.toFixed(1)} kHz`;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10_240 ? 1 : 0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function formatDuration(s: number): string {
  if (!isFinite(s)) return '—';
  if (s < 60) return `${Math.max(1, Math.round(s))} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min ${Math.round(s % 60)} s`;
  return `${Math.floor(s / 3600)} h ${Math.round((s % 3600) / 60)} min`;
}

export const MODES: { id: Mode; name: string; detail: string }[] = [
  {
    id: 'adaptive',
    name: 'Adaptive',
    detail: 'OFDM across your band, each tile tuned to the room. Kilobytes per frame; the format of dos net and dos duplex.',
  },
  {
    id: 'classic',
    name: 'Classic',
    detail: '100 baud FSK on 1.9/2.3 kHz with stop-and-wait, the original dos modem. About 15 bit/s, but works with minimodem.',
  },
];

export const LEVEL_NAMES = ['Pilot', 'Robust', 'QPSK', '8PSK'];
