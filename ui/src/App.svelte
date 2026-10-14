<script lang="ts">
  import { onMount } from 'svelte';
  import { Controller } from './controller.ts';
  import type { View } from './controller.ts';
  import type { Settings, Status } from './api.ts';

  let view = $state<View>({ saved: null, state: null, devices: [], deviceError: '', error: '', online: false, pending: 'Loading', notice: '' });
  let draft = $state<Settings | null>(null);
  let controller: Controller;
  const labels: Record<Status, string> = {
    stopped: 'Stopped', starting: 'Starting', listening: 'Listening for the sender',
    receiving: 'Receiving audio', sending: 'Sending UDP audio — remote reception is not confirmed',
    stopping: 'Stopping', failed: 'Failed — press Stop before restarting',
  };
  onMount(() => {
    controller = new Controller(next => {
      view = next;
      if (!draft && next.saved) draft = { ...next.saved };
    });
    void controller.initialize();
    return () => controller.dispose();
  });
  const locked = $derived(!!view.pending || !view.online || view.state?.status !== 'stopped');
  const devices = $derived(view.devices.filter(d => d.kind === (draft?.mode === 'send' ? 'capture' : 'playback')));
  const selectedId = $derived(draft?.mode === 'send' ? draft.capture_device_id : draft?.playback_device_id);
  const missing = $derived(!!selectedId && !devices.some(d => d.id === selectedId));
  const cable = $derived(view.devices.some(d => /cable/i.test(d.name)));
  const dirty = $derived(JSON.stringify(draft) !== JSON.stringify(view.saved));

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const action = (event.submitter as HTMLButtonElement | null)?.value === 'start' ? 'start' : 'save';
    void controller.mutate(action, draft);
  }
</script>

<main>
  <header><p class="eyebrow">LOCAL NETWORK AUDIO</p><h1>Audioportal</h1><p>Send audio to another computer, or listen alongside your local audio.</p></header>
  <section aria-labelledby="status-heading">
    <h2 id="status-heading">Stream</h2>
    <p role="status"><strong>{view.pending || (!view.online ? 'Backend unavailable — stream status unknown' : view.state ? labels[view.state.status] : 'Loading')}</strong></p>
    {#if view.error}<p role="alert" class="error">{view.error}</p>{/if}
    {#if view.online && view.state?.error}<p role="alert" class="error">{view.state.error.message}</p>{/if}
    {#if !view.online && !view.pending}<p>Run Audioportal, then reconnect. The displayed configuration is retained.</p>{/if}
    <div class="meters">
      <label>Input activity <meter min="0" max="1" value={view.online ? view.state?.input_peak ?? 0 : 0}></meter></label>
      <label>Output activity <meter min="0" max="1" value={view.online ? view.state?.output_peak ?? 0 : 0}></meter></label>
    </div>
    <p>Packets lost: {view.online ? view.state?.packets_lost ?? 0 : 'unavailable'} · Receive target: {view.online ? `${view.state?.buffer_ms ?? 0} ms` : 'unavailable'}</p>
    <div class="actions">
      <button type="button" disabled={!!view.pending || !view.online} onclick={() => controller.mutate('stop', draft)}>Stop</button>
      <button type="button" disabled={!!view.pending} onclick={() => controller.initialize()}>Reconnect / refresh devices</button>
    </div>
  </section>

  <section aria-labelledby="config-heading">
    <h2 id="config-heading">Configuration</h2>
    {#if draft}
      <form onsubmit={submit}>
        <fieldset disabled={locked}>
          <legend>Direction and destination</legend>
          <label for="mode">Mode</label>
          <select id="mode" bind:value={draft.mode}><option value="send">Send</option><option value="receive">Receive</option></select>
          <label>{draft.mode === 'send' ? 'Source device' : 'Headphones or speakers'}
            <select value={selectedId ?? ''} onchange={event => {
              if (!draft) return;
              const id = event.currentTarget.value || null;
              if (draft.mode === 'send') draft.capture_device_id = id;
              else draft.playback_device_id = id;
            }}>
              <option value="">Choose a device</option>
              {#if missing}<option value={selectedId}>Unavailable device: {selectedId}</option>{/if}
              {#each devices as device}<option value={device.id}>{device.name}</option>{/each}
            </select>
          </label>
          {#if view.deviceError}<p class="error" role="alert">{view.deviceError}</p>
          {:else if devices.length === 0}<p>No {draft.mode === 'send' ? 'capture' : 'playback'} devices found. Connect a device and refresh.</p>{/if}
          {#if missing}<p class="error">The selected device is unavailable. Reconnect it or choose another device.</p>{/if}
          <label>{draft.mode === 'send' ? "Receiver's IPv4 address" : "Sender's IPv4 address"}<input bind:value={draft.peer_ip} maxlength="64" placeholder="192.168.1.10" spellcheck="false" /></label>
          <details oninvalidcapture={event => { event.currentTarget.open = true; }}><summary>Advanced settings</summary>
            <label>UDP port<input type="number" bind:value={draft.udp_port} min="1" max="65535" step="1" required /></label>
            <label>Receive buffer (ms)<input type="number" bind:value={draft.buffer_ms} min="10" max="100" step="5" required /></label>
            <p>Use the same port on both computers. The receive buffer applies in Receive mode.</p>
          </details>
          <div class="actions"><button value="save" type="submit">Save</button><button class="primary" value="start" type="submit" disabled={!selectedId || missing || !!view.deviceError}>Save &amp; Start</button></div>
        </fieldset>
      </form>
      <p role="status">{dirty ? 'Unsaved changes' : view.notice || 'Settings saved'}</p>
      {#if view.online && view.state?.status !== 'stopped'}<p>Stop the session to edit configuration.</p>{/if}
    {:else}<p>Waiting for saved settings. If loading fails, fix the reported error and reconnect.</p>{/if}
  </section>

  <aside aria-labelledby="setup-heading"><h2 id="setup-heading">Setup</h2>
    <p>On the sender, route your apps to <strong>CABLE Input</strong> in Windows audio settings. Select the paired <strong>CABLE Output</strong> here as the source. Any compatible capture endpoint can be used.</p>
    {#if !cable}<p>No cable detected. <a href="https://vb-audio.com/Cable/" target="_blank" rel="noreferrer">Official VB-CABLE installation instructions</a> (administrator access and a reboot may be required).</p>{/if}
    <p>On the receiver, choose your headphones or speakers. Enter the other computer's IP on each machine. Start the receiver first, then the sender.</p>
  </aside>
</main>

<style>
  :global(body) { margin: 0; background: #f2f5f8; color: #182b3b; font: 1rem/1.55 system-ui, sans-serif; }
  :global(*) { box-sizing: border-box; }
  main { max-width: 760px; margin: auto; padding: 2rem 1rem 4rem; }
  h1 { font-size: 2.4rem; margin: 0; } h2 { font-size: 1.25rem; margin-top: 0; }
  .eyebrow { font-size: .8rem; letter-spacing: .12em; font-weight: 700; }
  section, aside { background: white; border: 1px solid #cbd5df; border-radius: .7rem; padding: 1.4rem; margin-top: 1.2rem; }
  fieldset { border: 0; margin: 0; padding: 0; min-width: 0; }
  legend { font-weight: 600; margin-bottom: .5rem; }
  label { display: block; margin: .8rem 0; }
  input, select { display: block; width: 100%; padding: .65rem; margin-top: .3rem; border: 1px solid #65798a; border-radius: .3rem; background: white; color: inherit; font: inherit; }
  button { padding: .65rem 1rem; border: 1px solid #65798a; border-radius: .3rem; background: white; color: #182b3b; font: inherit; cursor: pointer; }
  button.primary { background: #175580; color: white; border-color: #175580; }
  button:disabled, fieldset:disabled { opacity: .65; } button:disabled { cursor: default; }
  :global(:focus-visible) { outline: 3px solid #995500; outline-offset: 3px; }
  .actions { display: flex; gap: .75rem; flex-wrap: wrap; margin-top: 1rem; }
  .error { color: #a51e22; overflow-wrap: anywhere; }
  .meters { display: flex; gap: 2rem; flex-wrap: wrap; } meter { display: block; width: 100%; }
  a { color: #175580; } summary { cursor: pointer; font-weight: 600; }
</style>
