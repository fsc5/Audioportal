export type Mode = 'send' | 'receive';
export interface Device { id: string; name: string; kind: 'capture' | 'playback' }
export interface Settings {
  version: number;
  mode: Mode;
  capture_device_id: string | null;
  playback_device_id: string | null;
  peer_ip: string;
  udp_port: number;
  buffer_ms: number;
}
export type Status = 'stopped' | 'starting' | 'listening' | 'receiving' | 'sending' | 'stopping' | 'failed';
export interface ApiError { code: string; message: string }
export interface State {
  status: Status;
  mode: Mode | null;
  input_peak: number;
  output_peak: number;
  packets_lost: number;
  buffer_ms: number;
  error: ApiError | null;
}

const record = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null;
const mode = (v: unknown): v is Mode => v === 'send' || v === 'receive';
const nullableString = (v: unknown) => v === null || typeof v === 'string';
const integer = (v: unknown, min: number, max: number): v is number =>
  typeof v === 'number' && Number.isInteger(v) && v >= min && v <= max;
const apiError = (v: unknown): v is ApiError => record(v) && typeof v.code === 'string' && typeof v.message === 'string';
export const isSettings = (v: unknown): v is Settings => record(v) && v.version === 1 && mode(v.mode)
  && nullableString(v.capture_device_id) && nullableString(v.playback_device_id)
  && typeof v.peer_ip === 'string' && integer(v.udp_port, 1, 65535)
  && integer(v.buffer_ms, 10, 100) && v.buffer_ms % 5 === 0;
const peak = (v: unknown) => typeof v === 'number' && Number.isFinite(v) && v >= 0 && v <= 1;
const isState = (v: unknown): v is State => record(v)
  && ['stopped', 'starting', 'listening', 'receiving', 'sending', 'stopping', 'failed'].includes(String(v.status))
  && (v.mode === null || mode(v.mode)) && peak(v.input_peak) && peak(v.output_peak)
  && integer(v.packets_lost, 0, Number.MAX_VALUE) && integer(v.buffer_ms, 0, 100)
  && (v.error === null || apiError(v.error));
const isDevices = (v: unknown): v is { devices: Device[] } => record(v) && Array.isArray(v.devices)
  && v.devices.every(d => record(d) && typeof d.id === 'string' && typeof d.name === 'string'
    && (d.kind === 'capture' || d.kind === 'playback'));
const isBootstrap = (v: unknown): v is { csrf_token: string } => record(v)
  && typeof v.csrf_token === 'string' && v.csrf_token.length > 0;

export class RequestError extends Error {
  code: string;
  constructor(error: ApiError) { super(error.message); this.code = error.code; }
}

export class Api {
  token = '';
  fetcher: typeof fetch;
  constructor(fetcher: typeof fetch = (...args) => fetch(...args)) { this.fetcher = fetcher; }

  private async request<T>(path: string, validate: (v: unknown) => v is T, signal: AbortSignal,
    method = 'GET', settings?: Settings): Promise<T> {
    const response = await this.fetcher(`/api/${path}`, {
      method, signal: AbortSignal.any([signal, AbortSignal.timeout(10000)]), cache: 'no-store',
      headers: method === 'GET' ? {} : {
        'X-Audioportal-Token': this.token,
        ...(settings ? { 'Content-Type': 'application/json' } : {}),
      },
      body: settings ? JSON.stringify(settings) : undefined,
    });
    const body: unknown = await response.json();
    if (!response.ok) {
      if (record(body) && apiError(body.error)) throw new RequestError(body.error);
      throw new Error(`Backend request failed (HTTP ${response.status}).`);
    }
    if (!validate(body)) throw new Error('Backend returned an unexpected response. Restart Audioportal.');
    return body;
  }

  async bootstrap(signal: AbortSignal) { this.token = (await this.request('bootstrap', isBootstrap, signal)).csrf_token; }
  settings(signal: AbortSignal) { return this.request('settings', isSettings, signal); }
  devices(signal: AbortSignal) { return this.request('devices', isDevices, signal); }
  state(signal: AbortSignal) { return this.request('state', isState, signal); }
  save(settings: Settings, signal: AbortSignal) { return this.request('settings', isSettings, signal, 'PUT', settings); }
  start(signal: AbortSignal) { return this.request('start', isState, signal, 'POST'); }
  stop(signal: AbortSignal) { return this.request('stop', isState, signal, 'POST'); }
}
