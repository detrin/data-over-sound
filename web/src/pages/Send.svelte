<script lang="ts">
  import PacketStrip from '../components/PacketStrip.svelte';
  import TileMap from '../components/TileMap.svelte';
  import { estimate, prepare, send, stop } from '../lib/engine';
  import { app, formatBytes, formatDuration, formatHz, MODES, saveSettings, settings } from '../lib/state.svelte';
  import type { Estimate } from '../lib/types';

  let kind = $state<'message' | 'file'>('message');
  let text = $state('');
  let file = $state<File | null>(null);
  let eta = $state<Estimate | null>(null);
  let elapsed = $state(0);
  let fileInput = $state<HTMLInputElement>();

  const st = $derived(app.status?.role === 'leader' ? app.status : null);
  const cst = $derived(app.classic?.sending ? app.classic : null);
  const sending = $derived(app.activity === 'sending' && (st !== null || cst !== null));
  const classicProgress = $derived(cst && cst.total ? cst.packet / cst.total : 0);
  let classicEta = $state<number | null>(null);

  /** `legacy::airtime` for one packet with `payload` bytes (64 B packets, RS 16, 1 s turnaround). */
  function classicSeconds(bytes: number): number {
    const air = (payload: number) => {
      const raw = 9 + payload + 4 + 16;
      const framed = raw + Math.floor(raw / 254) + 2;
      return 1.0 + (11 + 4 * Math.ceil(framed / 3) + 10) * 0.1;
    };
    const packets = Math.ceil(bytes / 64);
    return packets * (air(64) + air(0) + 2.6) + air(0) * 2 + 2.6;
  }

  function pickMode(id: typeof settings.mode) {
    settings.mode = id;
    saveSettings();
  }
  const progress = $derived(st && st.send_total ? 1 - st.send_undelivered / st.send_total : 0);
  const payloadReady = $derived(kind === 'message' ? text.trim().length > 0 : file !== null);

  let token = 0;
  $effect(() => {
    const raw = kind === 'message' ? new TextEncoder().encode(text) : null;
    const size = raw ? raw.length : (file?.size ?? 0);
    void settings.lo;
    void settings.hi;
    const mine = ++token;
    if (!size) {
      eta = null;
      classicEta = null;
      return;
    }
    classicEta = classicSeconds(size);
    const sized = raw && size < 200_000 ? prepare(raw, true).then((p) => p.data.length) : Promise.resolve(size);
    sized.then((n) => estimate(n + 64)).then((e) => mine === token && (eta = e));
  });

  $effect(() => {
    if (!sending) return;
    const t = setInterval(() => (elapsed = (performance.now() - app.activityStarted) / 1000), 500);
    return () => clearInterval(t);
  });

  async function start() {
    elapsed = 0;
    if (kind === 'message') {
      await send(new TextEncoder().encode(text), { message: true, name: '', mime: 'text/plain', label: 'your message' });
    } else if (file) {
      await send(new Uint8Array(await file.arrayBuffer()), { message: false, name: file.name, mime: file.type, label: file.name });
    }
  }

  function again() {
    if (kind === 'message' && (st?.state === 'done' || cst?.state === 'done')) text = '';
    stop();
  }

  const phase = $derived.by(() => {
    if (!st) return 'Starting…';
    if (st.state === 'done') return 'Delivered. The other device confirmed every byte.';
    if (st.state === 'transmitting') return `Playing frame ${st.frames_sent} (${app.info?.frame_s.toFixed(1)} s of sound)`;
    if (st.state === 'listening') return st.frames_heard ? 'Listening for the other device’s answer' : 'Listening. The other device must be on Receive.';
    return 'Getting the next frame ready';
  });
</script>

{#if sending && cst}
  <section class="transfer" aria-live="polite">
    <h1>Sending {app.sentLabel}</h1>
    <p class="phase" class:done={cst.state === 'done'}>
      {cst.state === 'done'
        ? 'Delivered. Every packet and the end marker were confirmed.'
        : cst.state === 'failed'
          ? 'Not delivered: no confirmation after 5 retries.'
          : cst.state === 'transmitting'
            ? cst.packet + 1 >= cst.total
              ? 'Playing the end-of-transmission marker'
              : `Playing packet ${cst.packet + 1} of ${cst.total - 1}`
            : 'Waiting for the confirmation'}
    </p>
    <PacketStrip progress={classicProgress} active={cst.state !== 'done' && cst.state !== 'failed'} failed={cst.state === 'failed'} cells={Math.min(48, Math.max(8, cst.total))} label="Packets confirmed" />
    <dl class="stats">
      <div><dt>Packets confirmed</dt><dd class="num">{Math.min(cst.packet, cst.total - 1)}/{cst.total - 1}</dd></div>
      <div><dt>Retries</dt><dd class="num">{cst.retries}</dd></div>
      <div><dt>Mode</dt><dd>Classic</dd></div>
      <div><dt>Time</dt><dd class="num">{formatDuration(settings.simulate ? app.airSeconds : elapsed)}</dd></div>
    </dl>
    {#if cst.log.length}<p class="hint num">{cst.log.at(-1)}</p>{/if}
    {#if cst.state === 'done' || cst.state === 'failed'}
      <button class="btn btn-primary btn-wide" onclick={again}>Send something else</button>
    {:else}
      <button class="btn btn-quiet btn-wide" onclick={stop}>Stop sending</button>
    {/if}
  </section>
{:else if sending && st}
  <section class="transfer" aria-live="polite">
    <h1>Sending {app.sentLabel}</h1>
    <p class="phase" class:done={st.state === 'done'}>{phase}</p>

    <PacketStrip progress={progress} active={st.state !== 'done'} cells={48} label="Bytes confirmed by the receiver" />

    <dl class="stats">
      <div><dt>Confirmed</dt><dd class="num">{Math.round(progress * 100)}%</dd></div>
      <div><dt>Frames sent</dt><dd class="num">{st.frames_sent}</dd></div>
      <div><dt>Answers heard</dt><dd class="num">{st.frames_heard}</dd></div>
      <div><dt>Time</dt><dd class="num">{formatDuration(settings.simulate ? app.airSeconds : elapsed)}</dd></div>
    </dl>

    {#if app.info && st.out_levels.length}
      <div class="map">
        <h2>Tiles this device is sending on</h2>
        <p class="hint">The receiver picks these from what it heard. Darker tiles carry more bits.</p>
        <TileMap levels={st.out_levels} freqs={app.info.tile_freqs} label="Levels in use for sending" compact />
      </div>
    {/if}

    {#if st.frames_missed}
      <p class="hint">{st.frames_missed} answer{st.frames_missed === 1 ? '' : 's'} missed; the frame was sent again.</p>
    {/if}

    {#if st.state === 'done'}
      <button class="btn btn-primary btn-wide" onclick={again}>Send something else</button>
    {:else}
      <button class="btn btn-quiet btn-wide" onclick={stop}>Stop sending</button>
    {/if}
  </section>
{:else}
  <section class="compose">
    <div class="kind" role="tablist" aria-label="What to send">
      <button role="tab" aria-selected={kind === 'message'} class:on={kind === 'message'} onclick={() => (kind = 'message')}>Message</button>
      <button role="tab" aria-selected={kind === 'file'} class:on={kind === 'file'} onclick={() => (kind = 'file')}>File</button>
    </div>

    {#if kind === 'message'}
      <label class="field-label" for="msg">Message</label>
      <textarea id="msg" bind:value={text} rows="4" placeholder="Type a message for the other device."></textarea>
    {:else}
      <input bind:this={fileInput} type="file" class="visually-hidden" onchange={(e) => (file = e.currentTarget.files?.[0] ?? null)} />
      <button class="drop" onclick={() => fileInput?.click()}>
        {#if file}
          <strong>{file.name}</strong>
          <span class="hint">{formatBytes(file.size)}. Tap to choose another.</span>
        {:else}
          <strong>Choose a file</strong>
          <span class="hint">A quiet room moves a few KB per frame; small files arrive in minutes.</span>
        {/if}
      </button>
    {/if}

    <fieldset class="modes">
      <legend class="field-label">How to send</legend>
      {#each MODES as m (m.id)}
        <label class="mode" class:on={settings.mode === m.id}>
          <input type="radio" name="mode" value={m.id} checked={settings.mode === m.id} onchange={() => pickMode(m.id)} />
          <span class="mode-name">{m.name}</span>
          <span class="hint">{m.detail}</span>
        </label>
      {/each}
    </fieldset>

    {#if settings.mode === 'adaptive'}
      <p class="hint">
        {app.info?.frame_s.toFixed(1)} s frames on {formatHz(settings.lo)}–{formatHz(settings.hi)}; the other device answers each one.
        {#if eta}
          About <strong class="num">{formatDuration(eta.seconds_best)}</strong> in a quiet room, up to
          <span class="num">{formatDuration(eta.seconds_robust)}</span> in a noisy one.
        {/if}
      </p>
    {:else if classicEta !== null}
      <p class="hint">About <strong class="num">{formatDuration(classicEta)}</strong>: one 64-byte packet and its confirmation every ~23 s.</p>
    {/if}

    <p class="hint">Start <strong>Receive</strong> on the other device first.</p>

    <button class="btn btn-primary btn-wide" class:live={!settings.simulate} disabled={!payloadReady || !app.ready} onclick={start}>
      {kind === 'message' ? 'Send message' : 'Send file'}
    </button>
  </section>
{/if}

<style>
  .compose,
  .transfer {
    display: grid;
    gap: 18px;
  }

  .kind {
    display: grid;
    grid-template-columns: 1fr 1fr;
    padding: 4px;
    border-radius: calc(var(--radius-control) + 4px);
    background: var(--paper-sunk);
  }

  .kind button {
    min-height: var(--tap);
    border: 0;
    border-radius: var(--radius-control);
    background: transparent;
    font-weight: 700;
    color: var(--ink-soft);
  }

  .kind button.on {
    background: var(--paper-raised);
    color: var(--ink);
    box-shadow: 0 1px 2px rgb(34 40 58 / 0.18);
  }

  .compose > .field-label {
    margin-bottom: -10px;
  }

  textarea {
    width: 100%;
    min-height: 128px;
    padding: 14px 16px;
    border-radius: var(--radius-control);
    border: 1.5px solid var(--rule);
    background: var(--paper-raised);
    font-size: var(--s-18);
    line-height: 1.45;
    resize: vertical;
  }

  textarea:focus {
    border-color: var(--ink);
    outline: none;
  }

  .drop {
    display: grid;
    gap: 4px;
    justify-items: start;
    min-height: 112px;
    padding: 20px;
    border-radius: var(--radius-control);
    border: 1.5px dashed var(--ink-soft);
    background: var(--paper-raised);
    text-align: left;
    font-size: var(--s-18);
    overflow-wrap: anywhere;
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }

  .modes {
    display: grid;
    gap: 8px;
    margin: 0;
    padding: 0;
    border: 0;
  }

  .mode {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 12px;
    align-items: center;
    min-height: 64px;
    padding: 12px 16px;
    border-radius: var(--radius-control);
    border: 1.5px solid var(--rule);
    background: var(--paper-raised);
    cursor: pointer;
  }

  .mode.on {
    border-color: var(--ink);
    box-shadow: inset 4px 0 0 var(--ultra);
  }

  .mode input {
    grid-row: span 2;
    width: 22px;
    height: 22px;
    accent-color: var(--ultra);
    margin: 0;
  }

  .mode-name {
    font-weight: 700;
    font-size: var(--s-18);
  }

  .hint strong {
    color: var(--ink);
  }

  .transfer h1 {
    font-size: var(--s-28);
    overflow-wrap: anywhere;
  }

  h2 {
    font-size: var(--s-18);
  }

  .phase {
    margin-top: -10px;
    color: var(--ink-soft);
    font-size: var(--s-18);
  }

  .phase.done {
    color: var(--ultra-ink);
    font-weight: 700;
  }

  .stats {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 16px 12px;
    margin: 0;
  }

  .stats div {
    display: grid;
    gap: 2px;
  }

  .stats dt {
    color: var(--ink-soft);
    font-size: var(--s-14);
  }

  .stats dd {
    margin: 0;
    font-size: var(--s-28);
    font-weight: 600;
    line-height: 1.1;
  }

  .map {
    display: grid;
    gap: 6px;
  }

  @media (min-width: 420px) {
    .stats {
      grid-template-columns: repeat(4, 1fr);
    }
  }
</style>
