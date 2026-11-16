<script lang="ts">
  import { onMount } from 'svelte';
  import { onRows, VIEW_BINS } from '../lib/engine';
  import { app, settings, viewRange } from '../lib/state.svelte';

  const ROWS = 240;
  const HEARING = 17_500;

  let canvas: HTMLCanvasElement;
  let receivedAny = $state(false);
  let floor = -90;

  const view = $derived(viewRange());
  const pct = (hz: number) => ((hz - view.lo) / (view.hi - view.lo)) * 100;
  const ticks = $derived.by(() => {
    const span = view.hi - view.lo;
    const step = span > 12_000 ? 4_000 : span > 6_000 ? 2_000 : 1_000;
    const out: number[] = [];
    for (let f = Math.ceil(view.lo / step) * step; f <= view.hi; f += step) {
      const p = (f - view.lo) / (view.hi - view.lo);
      if (p > 0.04 && p < 0.96) out.push(f);
    }
    return out;
  });

  const PAPER = [232, 235, 238];
  const INK = [34, 40, 58];

  onMount(() => {
    const ctx = canvas.getContext('2d', { willReadFrequently: false })!;
    ctx.fillStyle = `rgb(${PAPER.join(',')})`;
    ctx.fillRect(0, 0, VIEW_BINS, ROWS);
    const off = onRows((data, bins) => {
      const n = Math.floor(data.length / bins);
      if (!n) return;
      receivedAny = true;
      const keep = Math.min(n, ROWS);
      ctx.drawImage(canvas, 0, 0, bins, ROWS - keep, 0, keep, bins, ROWS - keep);
      const img = ctx.createImageData(bins, keep);
      for (let r = 0; r < keep; r++) {
        const row = data.subarray((n - 1 - r) * bins, (n - r) * bins);
        const sorted = Array.from(row).sort((a, b) => a - b);
        floor = floor * 0.97 + sorted[Math.floor(bins * 0.3)] * 0.03;
        for (let b = 0; b < bins; b++) {
          const t = Math.min(1, Math.max(0, (row[b] - floor - 6) / 42)) ** 0.8;
          const o = (r * bins + b) * 4;
          img.data[o] = PAPER[0] + (INK[0] - PAPER[0]) * t;
          img.data[o + 1] = PAPER[1] + (INK[1] - PAPER[1]) * t;
          img.data[o + 2] = PAPER[2] + (INK[2] - PAPER[2]) * t;
          img.data[o + 3] = 255;
        }
      }
      ctx.putImageData(img, 0, 0);
    });
    return off;
  });

  const idleText = $derived(
    settings.simulate ? 'Loading the measured room…' : 'Waiting for the microphone. Tap Send or Start listening.',
  );
</script>

<figure class="waterfall" aria-label="Live spectrogram of the sound the microphone hears">
  <div class="plot">
    <canvas bind:this={canvas} width={VIEW_BINS} height={ROWS}></canvas>
    <div class="band" style:left="{pct(settings.lo)}%" style:width="{pct(settings.hi) - pct(settings.lo)}%">
      <span class="band-label">{app.info?.dos_default ? 'dos band' : 'your band'}</span>
    </div>
    {#if HEARING > view.lo && HEARING < view.hi}
      <div class="hearing" style:left="{pct(HEARING)}%">
        <span>edge of hearing</span>
      </div>
    {/if}
    {#if !receivedAny}
      <p class="idle">{idleText}</p>
    {/if}
    {#if app.status?.state === 'transmitting'}
      <span class="tx-flag">Transmitting</span>
    {/if}
  </div>
  <div class="axis" aria-hidden="true">
    {#each ticks as f (f)}
      <span class="num" style:left="{pct(f)}%">{f >= 1000 ? `${f / 1000}k` : f}</span>
    {/each}
  </div>
</figure>

<style>
  .waterfall {
    margin: 0;
    padding: 0 var(--pad);
  }

  .plot {
    position: relative;
    height: clamp(150px, 26vh, 300px);
    border-radius: var(--radius-sheet);
    overflow: hidden;
    background: var(--paper);
    box-shadow: inset 0 0 0 1.5px var(--rule);
  }

  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    image-rendering: auto;
  }

  .band {
    position: absolute;
    top: 0;
    bottom: 0;
    background: var(--ultra-wash);
    box-shadow: inset 1.5px 0 0 var(--ultra), inset -1.5px 0 0 var(--ultra);
    mix-blend-mode: multiply;
  }

  .band-label {
    position: absolute;
    top: 10px;
    left: 10px;
    font-size: var(--s-12);
    font-weight: 700;
    color: var(--ultra-ink);
    white-space: nowrap;
  }

  .hearing {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1.5px dashed var(--ink-soft);
  }

  .hearing span {
    position: absolute;
    top: 30px;
    right: 8px;
    font-size: var(--s-12);
    color: var(--ink-soft);
    white-space: nowrap;
  }

  .idle {
    position: absolute;
    inset: auto 16px 50% 16px;
    transform: translateY(50%);
    text-align: center;
    color: var(--ink-soft);
    font-size: var(--s-14);
    max-width: 30ch;
    margin: 0 auto;
  }

  .tx-flag {
    position: absolute;
    top: 10px;
    right: 10px;
    padding: 4px 10px;
    border-radius: 999px;
    background: var(--ultra);
    color: white;
    font-size: var(--s-12);
    font-weight: 700;
  }

  .axis {
    position: relative;
    height: 22px;
    margin-top: 4px;
  }

  .axis span {
    position: absolute;
    transform: translateX(-50%);
    font-size: var(--s-12);
    color: var(--ink-soft);
    white-space: nowrap;
  }
</style>
