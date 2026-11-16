<script lang="ts">
  import { onMount } from 'svelte';
  import Waterfall from './components/Waterfall.svelte';
  import { configure } from './lib/engine';
  import { app, settings, type Page, isBusy } from './lib/state.svelte';
  import Band from './pages/Band.svelte';
  import Link from './pages/Link.svelte';
  import Receive from './pages/Receive.svelte';
  import Send from './pages/Send.svelte';

  const NAV: { id: Page; label: string; icon: string }[] = [
    { id: 'send', label: 'Send', icon: 'M4 12h11M11 6l6 6-6 6M20 4v16' },
    { id: 'receive', label: 'Receive', icon: 'M20 12H9M13 6l-6 6 6 6M4 4v16' },
    { id: 'link', label: 'Link', icon: 'M4 6h3v12H4zM10.5 9h3v9h-3zM17 4h3v14h-3z' },
    { id: 'band', label: 'Band', icon: 'M3 12h3l2-6 4 12 3-9 2 3h4' },
  ];

  const busy = $derived(isBusy());

  function setSimulate(v: boolean) {
    if (settings.simulate === v || busy) return;
    settings.simulate = v;
    configure();
  }

  onMount(() => configure());

  let lastPage = app.page;
  $effect(() => {
    if (app.page !== lastPage) {
      lastPage = app.page;
      window.scrollTo({ top: 0 });
    }
  });
</script>

<div class="shell">
  <header class="top">
    <div class="brand">
      <svg viewBox="0 0 64 64" width="28" height="28" aria-hidden="true">
        <g fill="currentColor"><rect x="12" y="26" width="4" height="12" rx="2" /><rect x="20" y="18" width="4" height="28" rx="2" /><rect x="28" y="22" width="4" height="20" rx="2" /></g>
        <g fill="var(--ultra)"><rect x="38" y="14" width="4" height="36" rx="2" /><rect x="46" y="20" width="4" height="24" rx="2" /></g>
      </svg>
      <span>soundlink</span>
    </div>
    <div class="source" role="radiogroup" aria-label="Audio source">
      <button role="radio" aria-checked={settings.simulate} class:on={settings.simulate} disabled={busy} onclick={() => setSimulate(true)}>Simulated</button>
      <button role="radio" aria-checked={!settings.simulate} class:on={!settings.simulate} disabled={busy} onclick={() => setSimulate(false)}>Live</button>
    </div>
    <nav class="tabs-desktop" aria-label="Sections">
      {#each NAV as n (n.id)}
        <button class:current={app.page === n.id} aria-current={app.page === n.id ? 'page' : undefined} onclick={() => (app.page = n.id)}>{n.label}</button>
      {/each}
    </nav>
  </header>

  <div class="stage">
    <section class="scope">
      <Waterfall />
      {#if settings.simulate}
        <p class="sim-note hint">Simulated: a second device answers through a room measured between two laptops. Switch to Live for real speakers and mics.</p>
      {/if}
    </section>

    <main class="page">
      {#if app.error}
        <p class="error banner" role="alert">{app.error}</p>
      {/if}
      {#if app.page === 'send'}
        <Send />
      {:else if app.page === 'receive'}
        <Receive />
      {:else if app.page === 'link'}
        <Link />
      {:else}
        <Band />
      {/if}
    </main>
  </div>

  <nav class="tabs-mobile" aria-label="Sections">
    {#each NAV as n (n.id)}
      <button class:current={app.page === n.id} aria-current={app.page === n.id ? 'page' : undefined} onclick={() => (app.page = n.id)}>
        <svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><path d={n.icon} /></svg>
        <span>{n.label}</span>
        {#if n.id === 'receive' && app.activity === 'listening'}<i class="dot" aria-label="listening"></i>{/if}
        {#if n.id === 'send' && app.activity === 'sending'}<i class="dot" aria-label="sending"></i>{/if}
      </button>
    {/each}
  </nav>
</div>

<style>
  .shell {
    min-height: 100dvh;
    padding-bottom: calc(var(--nav-h) + var(--safe-b) + 24px);
  }

  .top {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: calc(env(safe-area-inset-top, 0px) + 10px) var(--pad) 10px;
    background: color-mix(in srgb, var(--paper) 88%, transparent);
    backdrop-filter: blur(10px);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-family: var(--display);
    font-weight: 700;
    font-size: var(--s-21);
    letter-spacing: -0.02em;
    font-variation-settings: 'wdth' 90;
  }

  .source {
    display: flex;
    padding: 3px;
    border-radius: 999px;
    background: var(--paper-sunk);
  }

  .source button {
    min-height: 44px;
    min-width: 44px;
    padding: 0 14px;
    border: 0;
    border-radius: 999px;
    background: transparent;
    font-size: var(--s-14);
    font-weight: 700;
    color: var(--ink-soft);
  }

  .source button.on {
    background: var(--paper-raised);
    color: var(--ink);
    box-shadow: 0 1px 2px rgb(34 40 58 / 0.18);
  }

  .source button:disabled:not(.on) {
    opacity: 0.5;
  }

  .stage {
    display: grid;
    gap: 20px;
  }

  .sim-note {
    padding: 8px var(--pad) 0;
    max-width: 62ch;
  }

  .page {
    padding: 0 var(--pad);
    max-width: 640px;
    width: 100%;
  }

  .banner {
    margin-bottom: 16px;
    padding: 12px 14px;
    border-radius: var(--radius-control);
    background: rgb(180 35 24 / 0.08);
  }

  .tabs-mobile {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 10;
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    height: calc(var(--nav-h) + var(--safe-b));
    padding: 6px 8px var(--safe-b);
    background: var(--paper-raised);
    box-shadow: 0 -1px 0 var(--rule);
  }

  .tabs-mobile button {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 2px;
    border: 0;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--ink-soft);
    font-size: var(--s-12);
    font-weight: 700;
  }

  .tabs-mobile svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .tabs-mobile button.current {
    color: var(--ink);
    background: var(--paper-sunk);
  }

  .dot {
    position: absolute;
    top: 8px;
    right: calc(50% - 20px);
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--ultra);
  }

  .tabs-desktop {
    display: none;
  }

  @media (min-width: 960px) {
    .shell {
      padding-bottom: 40px;
    }

    .tabs-mobile {
      display: none;
    }

    .tabs-desktop {
      display: flex;
      gap: 4px;
      margin-left: auto;
    }

    .tabs-desktop button {
      min-height: var(--tap);
      padding: 0 18px;
      border: 0;
      border-radius: var(--radius-control);
      background: transparent;
      font-weight: 700;
      color: var(--ink-soft);
    }

    .tabs-desktop button.current {
      background: var(--paper-sunk);
      color: var(--ink);
    }

    .stage {
      grid-template-columns: minmax(0, 1.25fr) minmax(380px, 1fr);
      align-items: start;
      gap: 8px;
      padding-top: 8px;
    }

    .scope {
      position: sticky;
      top: 76px;
    }

    .scope :global(.plot) {
      height: min(62vh, 560px);
    }
  }
</style>
