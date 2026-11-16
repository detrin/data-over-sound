<script lang="ts">
  import { formatHz, LEVEL_NAMES } from '../lib/state.svelte';

  let {
    levels,
    ok = null,
    snr = null,
    freqs,
    label,
    compact = false,
  }: { levels: number[]; ok?: boolean[] | null; snr?: number[] | null; freqs: number[]; label: string; compact?: boolean } = $props();

  const SNR_MAX = 30;
  let hover = $state<number | null>(null);
  const y = (v: number) => 40 - (Math.min(SNR_MAX, Math.max(0, v)) / SNR_MAX) * 40;
  function snrPath(area: boolean): string {
    if (!snr) return '';
    const pts = snr.map((v, i) => `${i + 0.5},${y(v).toFixed(2)}`).join(' L');
    return area ? `M0.5,40 L${pts} L${snr.length - 0.5},40 Z` : `M${pts}`;
  }
  const counts = $derived([0, 1, 2, 3].map((l) => levels.filter((v) => v === l).length));
  const failed = $derived(ok ? levels.filter((l, i) => l > 0 && !ok[i]).length : 0);
  const ticks = $derived.by(() => {
    if (!freqs.length) return [];
    const lo = freqs[0];
    const hi = freqs[freqs.length - 1];
    const step = hi - lo > 6000 ? 2000 : 1000;
    const out: { f: number; i: number }[] = [];
    for (let f = Math.ceil(lo / step) * step; f <= hi; f += step) {
      const i = freqs.findIndex((x) => x >= f);
      if (i > 2 && i < freqs.length - 3) out.push({ f, i });
    }
    return out;
  });
  const detail = $derived.by(() => {
    if (hover === null) return null;
    const l = levels[hover];
    const parts = [`${formatHz(freqs[hover])}`, LEVEL_NAMES[l] ?? `L${l}`];
    if (ok && l > 0) parts.push(ok[hover] ? 'decoded' : 'failed');
    if (snr) parts.push(`${snr[hover].toFixed(1)} dB`);
    return parts.join(', ');
  });
</script>

<figure class="tiles" class:compact aria-label={label}>
  <div class="grid" style:grid-template-columns="repeat({levels.length}, 1fr)" role="img" aria-label="{label}: {counts[3]} 8PSK, {counts[2]} QPSK, {counts[1]} robust, {counts[0]} pilot tiles{ok ? `, ${failed} failed` : ''}">
    {#each levels as l, i (i)}
      <button
        type="button"
        class="tile l{l}"
        class:fail={ok !== null && l > 0 && !ok[i]}
        class:hl={hover === i}
        onpointerenter={() => (hover = i)}
        onpointerdown={() => (hover = i)}
        onfocus={() => (hover = i)}
        aria-label="{formatHz(freqs[i])}: {LEVEL_NAMES[l]}"
        tabindex="-1"
      >
      </button>
    {/each}
  </div>
  {#if snr && !compact}
    <svg class="snr" viewBox="0 0 {levels.length} 40" preserveAspectRatio="none" aria-hidden="true">
      <line class="ref" x1="0" x2={levels.length} y1={40 - (20 / SNR_MAX) * 40} y2={40 - (20 / SNR_MAX) * 40} />
      <path class="area" d={snrPath(true)} />
      <path class="line" d={snrPath(false)} />
      {#if hover !== null}<line class="cursor" x1={hover + 0.5} x2={hover + 0.5} y1="0" y2="40" />{/if}
    </svg>
  {/if}
  <div class="axis num" aria-hidden="true">
    {#each ticks as t (t.f)}
      <span style:left="{((t.i + 0.5) / levels.length) * 100}%">{t.f / 1000}k</span>
    {/each}
  </div>
  {#if !compact}
    <figcaption>
      <ul class="legend">
        <li><i class="sw l3"></i>8PSK <span class="num">{counts[3]}</span></li>
        <li><i class="sw l2"></i>QPSK <span class="num">{counts[2]}</span></li>
        <li><i class="sw l1"></i>Robust <span class="num">{counts[1]}</span></li>
        <li><i class="sw l0"></i>Pilot <span class="num">{counts[0]}</span></li>
        {#if ok}<li><i class="sw fail-sw"></i>Failed <span class="num">{failed}</span></li>{/if}
      </ul>
      <p class="readout num">{detail ?? (snr ? 'Line: SNR of each tile, 0–30 dB (dashed at 20 dB). Tap a tile for details.' : 'Tap a tile for details.')}</p>
    </figcaption>
  {/if}
</figure>

<style>
  .tiles {
    margin: 0;
    display: grid;
    gap: 6px;
  }

  .grid {
    display: grid;
    gap: 1px;
    height: 48px;
    padding: 4px;
    border-radius: 12px;
    background: var(--paper-raised);
  }

  .compact .grid {
    height: 34px;
  }

  .tile {
    position: relative;
    min-width: 0;
    padding: 0;
    border: 0;
    border-radius: 2px;
    cursor: default;
    overflow: hidden;
  }

  .l0 {
    background: var(--paper-sunk);
  }
  .l1 {
    background: #a9b0bd;
  }
  .l2 {
    background: #6b7385;
  }
  .l3 {
    background: var(--ink);
  }

  .tile.fail {
    background-image: repeating-linear-gradient(135deg, var(--ultra) 0 2px, transparent 2px 5px);
  }

  .tile.hl {
    outline: 2px solid var(--ultra);
    outline-offset: 1px;
    z-index: 1;
  }

  .snr {
    display: block;
    width: 100%;
    height: 44px;
    padding: 0 4px;
    overflow: visible;
  }

  .snr .area {
    fill: var(--ultra-wash);
  }

  .snr .line {
    fill: none;
    stroke: var(--ultra);
    stroke-width: 2;
    vector-effect: non-scaling-stroke;
    stroke-linejoin: round;
  }

  .snr .ref {
    stroke: var(--rule);
    stroke-width: 1;
    stroke-dasharray: 3 3;
    vector-effect: non-scaling-stroke;
  }

  .snr .cursor {
    stroke: var(--ink);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }

  .axis {
    position: relative;
    height: 16px;
  }

  .axis span {
    position: absolute;
    transform: translateX(-50%);
    font-size: var(--s-12);
    color: var(--ink-soft);
  }

  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: var(--s-14);
  }

  .legend li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .legend .num {
    color: var(--ink-soft);
  }

  .sw {
    width: 14px;
    height: 14px;
    border-radius: 3px;
  }

  .fail-sw {
    background: repeating-linear-gradient(135deg, var(--ultra) 0 2px, var(--paper-raised) 2px 5px);
  }

  .readout {
    margin-top: 4px;
    min-height: 1.5em;
    font-size: var(--s-14);
    color: var(--ink-soft);
  }
</style>
