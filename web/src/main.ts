import '@fontsource-variable/bricolage-grotesque';
import '@fontsource/atkinson-hyperlegible/400.css';
import '@fontsource/atkinson-hyperlegible/700.css';
import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { app, settings } from './lib/state.svelte';

if (import.meta.env.DEV) (window as unknown as { __soundlink: unknown }).__soundlink = { app, settings };

export default mount(App, { target: document.getElementById('app')! });
