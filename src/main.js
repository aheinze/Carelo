import { createApp } from 'vue';
import { createPinia } from 'pinia';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import App from './App.vue';
import './assets/main.css';
import { vTooltip } from './directives/vTooltip';

const LEGACY_SETTINGS_KEY = 'carelo.phase1.settings';
const MIN_WINDOW_WIDTH = 960;
const MIN_WINDOW_HEIGHT = 640;
const MAX_WINDOW_DIMENSION = 10000;

function hasTauriRuntime() {
  return (
    typeof window !== 'undefined' &&
    (isTauri() || typeof window.__TAURI_INTERNALS__?.invoke === 'function')
  );
}

function normalizedDimension(value, min) {
  const number = Number(value);

  if (!Number.isFinite(number)) {
    return null;
  }

  return Math.max(min, Math.min(MAX_WINDOW_DIMENSION, Math.round(number)));
}

function legacyWindowDimensions() {
  try {
    const raw = window.localStorage.getItem(LEGACY_SETTINGS_KEY);
    const settings = raw ? JSON.parse(raw) : {};
    const dimensions = settings.windowDimensions || {};
    const width = normalizedDimension(dimensions.width, MIN_WINDOW_WIDTH);
    const height = normalizedDimension(dimensions.height, MIN_WINDOW_HEIGHT);

    return width && height ? { width, height } : null;
  } catch {
    return null;
  }
}

function clearLegacyWindowDimensions() {
  try {
    const raw = window.localStorage.getItem(LEGACY_SETTINGS_KEY);
    const settings = raw ? JSON.parse(raw) : {};
    delete settings.windowDimensions;
    window.localStorage.setItem(LEGACY_SETTINGS_KEY, JSON.stringify(settings));
  } catch {
    // Legacy settings cleanup is best-effort.
  }
}

async function migrateLegacyWindowDimensions() {
  if (!hasTauriRuntime()) {
    return;
  }

  const saved = await invoke('get_window_dimensions').catch(() => null);

  if (saved) {
    clearLegacyWindowDimensions();
    return;
  }

  const dimensions = legacyWindowDimensions();

  if (!dimensions) {
    return;
  }

  await getCurrentWindow()
    .setSize(new LogicalSize(dimensions.width, dimensions.height))
    .catch(() => {});
  await invoke('save_window_dimensions', dimensions).catch(() => {});
  clearLegacyWindowDimensions();
}

await migrateLegacyWindowDimensions();

const app = createApp(App);

app.use(createPinia());
app.directive('tooltip', vTooltip);
app.mount('#app');
