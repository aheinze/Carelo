import { LogicalSize } from '@tauri-apps/api/window';
import { onBeforeUnmount, onMounted } from 'vue';
import { loadUiSettings, saveUiSettings } from './useSettings';
import { getTauriWindow, hasTauriRuntime } from './useTauriWindow';

const MIN_WINDOW_WIDTH = 960;
const MIN_WINDOW_HEIGHT = 640;
const MAX_WINDOW_DIMENSION = 10000;
const RESIZE_SAVE_DELAY = 180;

let restorePromise = null;

function clampDimension(value, min) {
  const number = Number(value);

  if (!Number.isFinite(number)) {
    return null;
  }

  return Math.max(min, Math.min(MAX_WINDOW_DIMENSION, Math.round(number)));
}

function normalizedSavedDimensions() {
  const saved = loadUiSettings().windowDimensions || {};
  const width = clampDimension(saved.width, MIN_WINDOW_WIDTH);
  const height = clampDimension(saved.height, MIN_WINDOW_HEIGHT);

  if (!width || !height) {
    return null;
  }

  return { width, height };
}

export function saveCurrentWindowDimensions() {
  if (typeof window === 'undefined') {
    return;
  }

  const width = clampDimension(window.innerWidth, MIN_WINDOW_WIDTH);
  const height = clampDimension(window.innerHeight, MIN_WINDOW_HEIGHT);

  if (!width || !height) {
    return;
  }

  saveUiSettings({
    windowDimensions: {
      width,
      height,
      savedAt: Date.now(),
    },
  });
}

export function restoreWindowDimensions() {
  if (restorePromise) {
    return restorePromise;
  }

  restorePromise = (async () => {
    if (!hasTauriRuntime()) {
      return;
    }

    const dimensions = normalizedSavedDimensions();

    if (!dimensions) {
      return;
    }

    await getTauriWindow()?.setSize(new LogicalSize(dimensions.width, dimensions.height));
  })().catch(() => {});

  return restorePromise;
}

export function useWindowDimensionsPersistence() {
  let resizeTimer = 0;
  let unlistenClose = null;

  function saveSoon() {
    window.clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(saveCurrentWindowDimensions, RESIZE_SAVE_DELAY);
  }

  onMounted(async () => {
    if (!hasTauriRuntime()) {
      return;
    }

    window.addEventListener('resize', saveSoon);
    window.addEventListener('beforeunload', saveCurrentWindowDimensions);

    unlistenClose = await getTauriWindow()?.onCloseRequested(() => {
      window.clearTimeout(resizeTimer);
      saveCurrentWindowDimensions();
    });
  });

  onBeforeUnmount(() => {
    window.clearTimeout(resizeTimer);
    window.removeEventListener('resize', saveSoon);
    window.removeEventListener('beforeunload', saveCurrentWindowDimensions);
    unlistenClose?.();
  });
}
