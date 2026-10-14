import { Api, RequestError } from './api.ts';
import type { Device, Settings, State } from './api.ts';

export interface View {
  saved: Settings | null;
  state: State | null;
  devices: Device[];
  deviceError: string;
  error: string;
  online: boolean;
  pending: string;
  notice: string;
}
const message = (error: unknown) => error instanceof Error ? error.message : 'Request failed.';

// Owns request lifetimes; the editable draft belongs to the component, not polling.
export class Controller {
  view: View = { saved: null, state: null, devices: [], deviceError: '', error: '', online: false, pending: '', notice: '' };
  api: Api;
  changed: (view: View) => void;
  lifetime = new AbortController();
  pollRequest: AbortController | null = null;
  timer: ReturnType<typeof setTimeout> | undefined;

  constructor(changed: (view: View) => void, api = new Api()) { this.changed = changed; this.api = api; }
  private publish() { if (!this.lifetime.signal.aborted) this.changed({ ...this.view }); }
  private pause() { clearTimeout(this.timer); this.pollRequest?.abort(); }
  private schedule() {
    if (!this.lifetime.signal.aborted) this.timer = setTimeout(() => void this.poll(), 1000);
  }
  dispose() { this.pause(); this.lifetime.abort(); }

  async initialize() {
    if (this.view.pending || this.lifetime.signal.aborted) return;
    this.pause();
    this.view.pending = 'Loading';
    this.view.error = '';
    this.publish();
    try {
      if (!this.api.token) await this.api.bootstrap(this.lifetime.signal);
      this.view.state = await this.api.state(this.lifetime.signal);
      this.view.online = true;
      this.view.saved = await this.api.settings(this.lifetime.signal);
      await this.loadDevices();
    } catch (error) {
      this.view.error = message(error);
      this.view.online = false;
      this.view.state = null;
    } finally {
      this.view.pending = '';
      this.publish();
      this.schedule();
    }
  }

  private async loadDevices() {
    try {
      this.view.devices = (await this.api.devices(this.lifetime.signal)).devices;
      this.view.deviceError = '';
    } catch (error) {
      this.view.devices = [];
      this.view.deviceError = message(error);
    }
  }

  private async poll() {
    if (this.view.pending || this.lifetime.signal.aborted) return;
    const request = new AbortController();
    this.pollRequest = request;
    try {
      const state = await this.api.state(AbortSignal.any([request.signal, this.lifetime.signal]));
      if (!request.signal.aborted) { this.view.state = state; this.view.online = true; }
    } catch {
      if (!request.signal.aborted) { this.view.online = false; this.view.state = null; }
    } finally {
      if (!request.signal.aborted) { this.publish(); this.schedule(); }
    }
  }

  async mutate(action: 'save' | 'start' | 'stop', draft: Settings | null) {
    if (this.view.pending || !this.view.online || this.lifetime.signal.aborted) return;
    if (action !== 'stop' && (!draft || this.view.state?.status !== 'stopped')) return;
    this.pause();
    this.view.pending = action === 'start' ? 'Saving and starting' : action === 'save' ? 'Saving' : 'Stopping';
    this.view.error = '';
    this.view.notice = '';
    this.publish();
    try {
      if (action !== 'stop' && draft) {
        this.view.saved = await this.api.save({ ...draft }, this.lifetime.signal);
        this.view.notice = 'Settings saved.';
        this.publish();
      }
      if (action === 'start') this.view.state = await this.api.start(this.lifetime.signal);
      if (action === 'stop') this.view.state = await this.api.stop(this.lifetime.signal);
    } catch (error) {
      this.view.error = message(error);
      // The API has no dedicated token error code. Refresh once for invalid_request,
      // but never replay a mutation whose outcome might be uncertain.
      if (error instanceof RequestError && error.code === 'invalid_request') {
        try {
          await this.api.bootstrap(this.lifetime.signal);
          this.view.error += ' API token refreshed; review settings and retry.';
        } catch (refreshError) { this.view.error += ` ${message(refreshError)}`; }
      }
    } finally {
      try {
        this.view.state = await this.api.state(this.lifetime.signal);
        this.view.online = true;
      } catch { this.view.online = false; this.view.state = null; }
      this.view.pending = '';
      this.publish();
      this.schedule();
    }
  }
}
