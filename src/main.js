import { createApp, nextTick } from 'vue';
import { createPinia } from 'pinia';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import App from './App.vue';
import './assets/main.css';
import { vTooltip } from './directives/vTooltip';
import { applyAccentColor, applyColorScheme } from './utils/colorSchemes';

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

function applyAppearanceMode(mode) {
  if (mode === 'light' || mode === 'dark') {
    document.documentElement.dataset.theme = mode;
  } else {
    document.documentElement.removeAttribute('data-theme');
  }
}

function legacyAppearanceMode() {
  try {
    const raw = window.localStorage.getItem(LEGACY_SETTINGS_KEY);
    const settings = raw ? JSON.parse(raw) : {};
    return settings.appSettings?.appearanceMode || null;
  } catch {
    return null;
  }
}

function legacyColorScheme() {
  try {
    const raw = window.localStorage.getItem(LEGACY_SETTINGS_KEY);
    const settings = raw ? JSON.parse(raw) : {};
    return settings.appSettings?.colorScheme || null;
  } catch {
    return null;
  }
}

function legacyAccentColor() {
  try {
    const raw = window.localStorage.getItem(LEGACY_SETTINGS_KEY);
    const settings = raw ? JSON.parse(raw) : {};
    return settings.appSettings?.accentColor || null;
  } catch {
    return null;
  }
}

async function applyStoredAppearanceMode() {
  applyAppearanceMode(legacyAppearanceMode());
  applyColorScheme(legacyColorScheme());
  applyAccentColor(legacyAccentColor());

  if (!hasTauriRuntime()) {
    return;
  }

  const settings = await invoke('get_app_settings').catch(() => null);
  applyAppearanceMode(settings?.appearanceMode || legacyAppearanceMode());
  applyColorScheme(settings?.colorScheme || legacyColorScheme());
  applyAccentColor(settings?.accentColor ?? legacyAccentColor());
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

async function showMainWindowWhenReady() {
  if (!hasTauriRuntime()) {
    return;
  }

  await nextTick();
  await getCurrentWindow().show().catch(() => {});
}

await applyStoredAppearanceMode();
await migrateLegacyWindowDimensions();

const app = createApp(App);

app.use(createPinia());
app.directive('tooltip', vTooltip);
app.mount('#app');
await showMainWindowWhenReady();
