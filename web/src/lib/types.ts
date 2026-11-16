export const SAMPLE_RATE = 48_000;

// envelope flags (wasm/src/station.rs)
export const FLAG_MESSAGE = 1;
export const FLAG_GZIP = 2;

export type Role = 'leader' | 'follower';
export type Mode = 'adaptive' | 'classic';
export type StationState = 'ready' | 'transmitting' | 'listening' | 'done';

export interface BandInfo {
  f_lo: number;
  f_hi: number;
  dos_default: boolean;
  n_tiles: number;
  max_tiles: number;
  tile_hz: number;
  tile_freqs: number[];
  frame_s: number;
  tile_payload: number[];
  frame_payload_robust: number;
  frame_payload_best: number;
}

export interface Estimate {
  rounds_best: number;
  rounds_robust: number;
  seconds_best: number;
  seconds_robust: number;
}

export interface LastFrame {
  header_ok: boolean;
  seq: number | null;
  levels: number[];
  tile_ok: boolean[];
  tile_snr_db: number[];
  payload_bytes: number;
  snr_median_db: number;
}

export interface StationStatus {
  role: Role;
  state: StationState;
  frames_sent: number;
  frames_heard: number;
  frames_missed: number;
  false_alarms: number;
  send_total: number;
  send_undelivered: number;
  recv_total: number;
  recv_progress: number;
  out_levels: number[];
  last: LastFrame | null;
  log: string[];
}

export interface ClassicStatus {
  sending: boolean;
  state: 'ready' | 'transmitting' | 'listening' | 'done' | 'failed';
  packet: number;
  total: number;
  retries: number;
  received_bytes: number;
  frames_heard: number;
  log: string[];
}

export interface SimSetup {
  speed: number;
  scenario: number;
}

export interface View {
  lo: number;
  hi: number;
  bins: number;
}

export interface Band {
  lo: number;
  hi: number;
}

export type ToWorker =
  | { t: 'config'; band: Band; simulate: boolean; sim: SimSetup; view: View }
  | { t: 'send'; mode: Mode; name: string; mime: string; flags: number; data: ArrayBuffer }
  | { t: 'listen' }
  | { t: 'stop' }
  | { t: 'scenario'; index: number }
  | { t: 'mic_port'; port: MessagePort }
  | { t: 'played' };

export interface CompletedMsg {
  name: string;
  mime: string;
  flags: number;
  data: ArrayBuffer;
}

export type FromWorker =
  | { t: 'ready'; info: BandInfo; scenarios: string[] }
  | { t: 'error'; message: string }
  | { t: 'rows'; data: Float32Array; bins: number }
  | { t: 'status'; status: StationStatus | null; classic: ClassicStatus | null; level: number; air_s: number }
  | { t: 'play'; samples: Float32Array }
  | { t: 'completed'; file: CompletedMsg }
  | { t: 'idle' };
