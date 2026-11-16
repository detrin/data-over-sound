<script lang="ts">
  import PacketStrip from '../components/PacketStrip.svelte';
  import TileMap from '../components/TileMap.svelte';
  import { listen, stop } from '../lib/engine';
  import { app, formatBytes, formatHz, settings, type Received } from '../lib/state.svelte';

  const st = $derived(app.status?.role === 'follower' ? app.status : null);
  const listening = $derived(app.activity === 'listening');
  const meter = $derived(Math.min(1, Math.max(0, (20 * Math.log10(app.level + 1e-9) + 66) / 54)));
  const arriving = $derived(st && st.recv_total > 0 && st.recv_progress < 1 ? st : null);
  let copied = $state<string | null>(null);

  async function copy(r: Received) {
    if (!r.message) return;
    await navigator.clipboard?.writeText(r.message);
    copied = r.id;
    setTimeout(() => (copied = null), 1500);
  }

  const canShare = () => typeof navigator.share === 'function' && typeof navigator.canShare === 'function';

  async function share(r: Received) {
    if (!r.url) return;
    const blob = await (await fetch(r.url)).blob();
    const file = new File([blob], r.name || 'soundlink-file', { type: r.mime || blob.type });
    if (navigator.canShare?.({ files: [file] })) await navigator.share({ files: [file] }).catch(() => {});
  }

  const time = (d: Date) => d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const cl = $derived(app.classic && !app.classic.sending ? app.classic : null);
  const classicArriving = $derived(cl && cl.received_bytes > 0 && !cl.log.at(-1)?.includes('end of transmission') ? cl : null);
  const latest = $derived(st?.log.at(-1) ?? cl?.log.at(-1) ?? null);
</script>

<section class="receive">
  {#if listening}
    <div class="listening">
      <div class="meter" aria-hidden="true"><span style:transform="scaleX({meter})"></span></div>
      <p>
        Listening for adaptive frames on <strong>{formatHz(settings.lo)}–{formatHz(settings.hi)}</strong> and classic packets on 1.9/2.3 kHz.
        Every one heard gets an answer, so keep the speaker on and this screen open.
      </p>
      {#if latest}<p class="hint num">{latest}</p>{/if}
    </div>
    <button class="btn btn-quiet btn-wide" onclick={stop}>Stop listening</button>
  {:else}
    <h1>Receive</h1>
    <p class="hint lead">Start listening here, then send from the other device. Both must use the same band.</p>
    <button class="btn btn-primary btn-wide" class:live={!settings.simulate} disabled={!app.ready} onclick={listen}>Start listening</button>
  {/if}

  {#if arriving}
    <article class="incoming">
      <header>
        <strong>Arriving</strong>
        <span class="hint num">{formatBytes(Math.round(arriving.recv_progress * arriving.recv_total))} of {formatBytes(arriving.recv_total)}</span>
      </header>
      <PacketStrip progress={arriving.recv_progress} active={true} cells={32} label="Bytes received" />
    </article>
  {/if}

  {#if classicArriving}
    <article class="incoming">
      <header>
        <strong>Arriving by classic modem</strong>
        <span class="hint num">{classicArriving.packet} packets, {formatBytes(classicArriving.received_bytes)}</span>
      </header>
      <p class="hint">Stop-and-wait: one 64-byte packet at a time, about 23 s each. The total size arrives with the last one.</p>
    </article>
  {/if}

  {#if listening && st?.last && app.info}
    <div class="frame">
      <h2>Last frame heard</h2>
      <p class="hint num">
        {st.last.header_ok ? `${st.last.tile_ok.filter(Boolean).length} of ${st.last.tile_ok.length} tiles decoded, median SNR ${st.last.snr_median_db.toFixed(1)} dB` : 'Header lost; the sender will repeat it.'}
      </p>
      {#if st.last.levels.length}
        <TileMap levels={st.last.levels} ok={st.last.tile_ok} snr={st.last.tile_snr_db} freqs={app.info.tile_freqs} label="Last frame, tile by tile" />
      {/if}
    </div>
  {/if}

  <div class="group">
    <h2>Received</h2>
    {#if !app.received.length}
      <p class="hint">Nothing yet. Messages appear here, and files are ready to save.</p>
    {:else}
      <ul class="items">
        {#each app.received as r (r.id)}
          <li>
            {#if r.message !== null}
              <blockquote>{r.message}</blockquote>
              <div class="row">
                <span class="hint">Message at {time(r.at)}</span>
                <button class="btn btn-quiet small" onclick={() => copy(r)}>{copied === r.id ? 'Copied' : 'Copy'}</button>
              </div>
            {:else}
              <div class="file">
                <strong>{r.name || 'Unnamed file'}</strong>
                <span class="hint">{formatBytes(r.size)} at {time(r.at)}</span>
              </div>
              <div class="row actions">
                <a class="btn btn-primary small" href={r.url} download={r.name || 'soundlink-file'}>Save</a>
                {#if canShare()}
                  <button class="btn btn-quiet small" onclick={() => share(r)}>Share</button>
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</section>

<style>
  .receive {
    display: grid;
    gap: 18px;
  }

  h1 {
    font-size: var(--s-28);
  }

  .lead {
    margin-top: -10px;
    font-size: var(--s-16);
  }

  h2 {
    font-size: var(--s-21);
  }

  .listening {
    display: grid;
    gap: 10px;
    font-size: var(--s-18);
  }

  .listening .hint {
    font-size: var(--s-14);
  }

  .meter {
    height: 10px;
    border-radius: 999px;
    background: var(--paper-sunk);
    overflow: hidden;
  }

  .meter span {
    display: block;
    height: 100%;
    background: var(--ultra);
    transform-origin: left;
    transition: transform 160ms linear;
  }

  .incoming {
    display: grid;
    gap: 10px;
    padding: 16px;
    border-radius: var(--radius-control);
    border: 1.5px solid var(--ink);
  }

  .incoming header {
    display: flex;
    justify-content: space-between;
    gap: 10px;
  }

  .frame,
  .group {
    display: grid;
    gap: 10px;
  }

  .items {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 10px;
  }

  .items li {
    display: grid;
    gap: 10px;
    padding: 16px;
    border-radius: var(--radius-control);
    background: var(--paper-raised);
  }

  blockquote {
    margin: 0;
    font-size: var(--s-18);
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .file {
    display: grid;
    overflow-wrap: anywhere;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .actions {
    justify-content: flex-start;
  }

  .small {
    min-height: 44px;
    padding: 0 16px;
    font-size: var(--s-14);
    text-decoration: none;
  }

  a.btn-primary.small {
    min-height: 44px;
    font-size: var(--s-14);
  }
</style>
