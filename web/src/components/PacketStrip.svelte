<script lang="ts">
  let {
    progress,
    active = false,
    failed = false,
    cells = 32,
    label,
  }: { progress: number; active?: boolean; failed?: boolean; cells?: number; label: string } = $props();

  const filled = $derived(Math.min(cells, Math.floor(progress * cells + 1e-6)));
</script>

<div class="strip" class:failed role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(progress * 100)}>
  {#each { length: cells } as _, i (i)}
    <span class="cell" class:on={i < filled} class:next={active && i === filled}></span>
  {/each}
</div>

<style>
  .strip {
    display: grid;
    grid-template-columns: repeat(16, 1fr);
    gap: 4px;
  }

  .cell {
    aspect-ratio: 1.6;
    border-radius: 4px;
    background: var(--paper-sunk);
    transition: background 200ms;
  }

  .cell.on {
    background: var(--ink);
  }

  .failed .cell.on {
    background: var(--ink-soft);
  }

  .cell.next {
    background: var(--ultra);
    animation: blink 900ms ease-in-out infinite alternate;
  }

  @keyframes blink {
    from {
      opacity: 1;
    }
    to {
      opacity: 0.3;
    }
  }
</style>
