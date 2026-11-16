<script lang="ts">
  import { bandInfo, configure } from '../lib/engine';
  import { app, formatBytes, formatHz, PRESETS, settings, isBusy } from '../lib/state.svelte';
  import type { BandInfo } from '../lib/types';

  let lo = $state(settings.lo);
  let hi = $state(settings.hi);
  let volume = $state(settings.volume);
  let speed = $state(settings.simSpeed);
  let preview = $state<BandInfo | string | null>(null);

  const busy = $derived(isBusy());
  const changed = $derived(lo !== settings.lo || hi !== settings.hi || volume !== settings.volume || speed !== settings.simSpeed);
  const presetId = $derived(PRESETS.find((p) => p.lo === lo && p.hi === hi)?.id ?? null);
  const info = $derived(typeof preview === 'object' ? preview : null);
  const problem = $derived(typeof preview === 'string' ? preview : null);

  $effect(() => {
    const [a, b] = [lo, hi];
    let live = true;
    bandInfo(a, b).then((p) => live && (preview = p));
    return () => (live = false);
  });

  const RULER_HI = 24_000;
  const pct = (f: number) => (f / RULER_HI) * 100;

  function apply() {
    Object.assign(settings, { lo, hi, volume, simSpeed: speed });
    configure();
  }

  function revert() {
    lo = settings.lo;
    hi = settings.hi;
    volume = settings.volume;
    speed = settings.simSpeed;
  }
</script>

<section class="band">
  <h1>Band</h1>
  <p class="hint lead">Both devices must use the same band. Standard matches the dos program, so a phone can talk to a laptop running it.</p>

  <div class="presets" role="radiogroup" aria-label="Band presets">
    {#each PRESETS as p (p.id)}
      <button role="radio" aria-checked={presetId === p.id} class:on={presetId === p.id} onclick={() => ((lo = p.lo), (hi = p.hi))}>
        <strong>{p.name}</strong>
        <span class="hint">{p.detail}</span>
      </button>
    {/each}
  </div>

  <div class="ruler" aria-hidden="true">
    <div class="track">
      <div class="audible" style:width="{pct(17_500)}%"><span>most adults hear this</span></div>
      <div class="sel" style:left="{pct(lo)}%" style:width="{pct(hi) - pct(lo)}%">
        {#each info?.tile_freqs ?? [] as f, i (i)}<i style:left="{((f - lo) / (hi - lo)) * 100}%"></i>{/each}
      </div>
    </div>
    <div class="scale num">
      {#each [0, 6, 12, 18, 24] as k (k)}<span style:left="{pct(k * 1000)}%">{k}k</span>{/each}
    </div>
  </div>

  <div class="summary" aria-live="polite">
    {#if info}
      <p>
        <strong class="num">{info.n_tiles}</strong> tiles of {formatHz(info.tile_hz)}. A {info.frame_s.toFixed(1)} s frame carries
        <strong class="num">{formatBytes(info.frame_payload_best)}</strong> when every tile is clean and
        <span class="num">{formatBytes(info.frame_payload_robust)}</span> on the robust fallback.
      </p>
      {#if !info.dos_default}
        <p class="hint">Only Standard is compatible with the dos program; any other band only talks to this app on the same band.</p>
      {/if}
    {:else if problem}
      <p class="error">{problem}</p>
    {/if}
  </div>

  <div class="sliders">
    <label>
      <span class="field-label">Lowest frequency <span class="num value">{formatHz(lo)}</span></span>
      <input type="range" min="300" max="22000" step="100" value={lo} oninput={(e) => (lo = Math.min(+e.currentTarget.value, hi - 1000))} />
    </label>
    <label>
      <span class="field-label">Highest frequency <span class="num value">{formatHz(hi)}</span></span>
      <input type="range" min="1300" max="23500" step="100" value={hi} oninput={(e) => (hi = Math.max(+e.currentTarget.value, lo + 1000))} />
    </label>
    <label>
      <span class="field-label">Speaker volume <span class="num value">{Math.round(volume * 100)}%</span></span>
      <input type="range" min="0.1" max="1" step="0.05" bind:value={volume} />
    </label>
  </div>

  {#if settings.simulate}
    <div>
      <span class="field-label" id="spd">Simulation clock</span>
      <div class="seg" role="radiogroup" aria-labelledby="spd">
        {#each [1, 2, 4] as s (s)}
          <button role="radio" aria-checked={speed === s} class:on={speed === s} onclick={() => (speed = s)}><span class="num">{s}×</span></button>
        {/each}
      </div>
      <p class="hint">Fast-forwards the simulated air. A real exchange is one frame each way, about 16 s.</p>
    </div>
  {/if}

  {#if changed}
    <div class="apply">
      <button class="btn btn-primary" disabled={!info || busy} onclick={apply}>Apply</button>
      <button class="btn btn-quiet" onclick={revert}>Undo changes</button>
    </div>
    {#if busy}<p class="hint">Stop the current transfer before changing the band.</p>{/if}
  {/if}
</section>

<style>
  .band {
    display: grid;
    gap: 20px;
  }

  h1 {
    font-size: var(--s-28);
  }

  .lead {
    margin-top: -12px;
    font-size: var(--s-16);
  }

  .presets {
    display: grid;
    gap: 8px;
  }

  .presets button {
    display: grid;
    gap: 2px;
    justify-items: start;
    min-height: 64px;
    padding: 12px 16px;
    text-align: left;
    border-radius: var(--radius-control);
    border: 1.5px solid var(--rule);
    background: var(--paper-raised);
  }

  .presets button strong {
    font-size: var(--s-18);
  }

  .presets button.on {
    border-color: var(--ink);
    box-shadow: inset 4px 0 0 var(--ultra);
  }

  .track {
    position: relative;
    height: 56px;
    border-radius: var(--radius-control);
    background: var(--paper-raised);
    overflow: hidden;
  }

  .audible {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    background: repeating-linear-gradient(135deg, transparent 0 7px, rgb(34 40 58 / 0.07) 7px 9px);
  }

  .audible span {
    position: absolute;
    left: 10px;
    top: 8px;
    font-size: var(--s-12);
    color: var(--ink-soft);
  }

  .sel {
    position: absolute;
    top: 0;
    bottom: 0;
    background: var(--ultra-wash);
    box-shadow: inset 1.5px 0 0 var(--ultra), inset -1.5px 0 0 var(--ultra);
  }

  .sel i {
    position: absolute;
    bottom: 0;
    width: 1px;
    height: 14px;
    background: var(--ultra);
    opacity: 0.6;
  }

  .scale {
    position: relative;
    height: 20px;
    margin-top: 4px;
  }

  .scale span {
    position: absolute;
    transform: translateX(-50%);
    font-size: var(--s-12);
    color: var(--ink-soft);
  }

  .scale span:first-child {
    transform: none;
  }

  .scale span:last-child {
    transform: translateX(-100%);
  }

  .summary {
    display: grid;
    gap: 6px;
    font-size: var(--s-18);
    margin-top: -6px;
  }

  .summary .hint {
    font-size: var(--s-14);
  }

  .sliders {
    display: grid;
    gap: 16px;
  }

  .sliders .field-label {
    display: flex;
    justify-content: space-between;
  }

  .value {
    font-weight: 600;
  }

  input[type='range'] {
    width: 100%;
    height: var(--tap);
    margin: 0;
    accent-color: var(--ultra);
  }

  .seg {
    display: flex;
    gap: 6px;
    margin-bottom: 6px;
  }

  .seg button {
    flex: 1 1 0;
    min-height: var(--tap);
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

  .apply {
    position: sticky;
    bottom: calc(var(--nav-h) + var(--safe-b) + 12px);
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 10px;
    padding: 10px;
    border-radius: calc(var(--radius-control) + 6px);
    background: var(--paper-raised);
    box-shadow: 0 6px 24px rgb(34 40 58 / 0.18);
  }

  @media (min-width: 960px) {
    .apply {
      bottom: 16px;
    }
  }
</style>
