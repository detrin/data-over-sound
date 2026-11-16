<script lang="ts">
  import TileMap from '../components/TileMap.svelte';
  import { listen, setScenario } from '../lib/engine';
  import { app, formatHz, settings } from '../lib/state.svelte';

  const st = $derived(app.status);
  const last = $derived(st?.last ?? null);
  const info = $derived(app.info);
  const active = $derived(app.activity !== 'idle');

  const ROOM_LABELS: Record<string, string> = {
    quiet: 'Quiet',
    'jam 2-4 kHz': 'Jammer 2–4 kHz',
    'jam 6-9 kHz': 'Jammer 6–9 kHz',
    'broadband +14dB': 'Broadband noise',
    'bursts 1-5 kHz': 'Bursts 1–5 kHz',
  };
  const rooms = $derived(app.scenarios.map((name, i) => ({ i, name, label: ROOM_LABELS[name] })).filter((r) => r.label));
</script>

<section class="link">
  <h1>Link</h1>
  <p class="hint lead">
    The band is split into {info?.n_tiles ?? '…'} tiles of {info ? formatHz(info.tile_hz) : '…'}. After every frame the receiver measures
    each tile and tells the sender how many bits that tile should carry next, so noisy parts of the band fall back while clean ones speed
    up.
  </p>

  {#if settings.simulate}
    <div>
      <span class="field-label" id="room">Simulated room noise</span>
      <div class="seg" role="radiogroup" aria-labelledby="room">
        {#each rooms as r (r.i)}
          <button role="radio" aria-checked={settings.simScenario === r.i} class:on={settings.simScenario === r.i} onclick={() => setScenario(r.i)}>{r.label}</button>
        {/each}
      </div>
      <p class="hint">Measured room between two laptops, plus the interference picked here. Change it mid-transfer and watch the tiles adapt.</p>
    </div>
  {/if}

  {#if !active}
    <div class="empty">
      <p>The link view fills in during a transfer.</p>
      {#if settings.simulate}
        <button class="btn btn-primary btn-wide" disabled={!app.ready} onclick={listen}>Watch a simulated transfer</button>
      {:else}
        <p class="hint">Start Send or Receive, then come back here.</p>
      {/if}
    </div>
  {/if}

  {#if active && info}
    {#if last && last.levels.length}
      <div class="block">
        <h2>What this device heard</h2>
        <p class="hint num">
          Frame {last.seq ?? '?'}: {last.tile_ok.filter(Boolean).length}/{last.tile_ok.length} tiles decoded, {last.payload_bytes} B,
          median SNR {last.snr_median_db.toFixed(1)} dB
        </p>
        <TileMap levels={last.levels} ok={last.tile_ok} snr={last.tile_snr_db} freqs={info.tile_freqs} label="Last frame heard" />
      </div>
    {:else}
      <p class="hint">Waiting for the first frame from the other device…</p>
    {/if}

    {#if st && st.out_levels.length}
      <div class="block">
        <h2>What this device sends with</h2>
        <p class="hint">Chosen by the other device from what it heard{st.role === 'follower' ? '. A receiver only answers, so its tiles carry acknowledgements' : ''}.</p>
        <TileMap levels={st.out_levels} freqs={info.tile_freqs} label="Levels this device transmits with" compact />
      </div>
    {/if}

    {#if st?.log.length}
      <div class="block">
        <h2>Frames</h2>
        <ul class="log num">
          {#each st.log.slice(-8).reverse() as line, i (i)}<li>{line}</li>{/each}
        </ul>
      </div>
    {/if}
  {/if}
</section>

<style>
  .link {
    display: grid;
    gap: 20px;
  }

  h1 {
    font-size: var(--s-28);
  }

  h2 {
    font-size: var(--s-18);
  }

  .lead {
    margin-top: -12px;
    font-size: var(--s-16);
    max-width: 62ch;
  }

  .seg {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 6px;
  }

  .seg button {
    flex: 1 1 auto;
    min-height: var(--tap);
    padding: 0 12px;
    border-radius: var(--radius-control);
    border: 1.5px solid var(--rule);
    background: var(--paper-raised);
    font-weight: 700;
  }

  .seg button.on {
    background: var(--ink);
    border-color: var(--ink);
    color: var(--paper-raised);
  }

  .empty {
    display: grid;
    gap: 12px;
    padding: 18px;
    border-radius: var(--radius-control);
    background: var(--paper-raised);
  }

  .block {
    display: grid;
    gap: 8px;
  }

  .log {
    margin: 0;
    padding: 12px 14px 12px 2.2em;
    border-radius: var(--radius-control);
    background: var(--paper-raised);
    font-size: var(--s-14);
    color: var(--ink-soft);
    display: grid;
    gap: 4px;
  }

  .log li:first-child {
    color: var(--ink);
  }
</style>
