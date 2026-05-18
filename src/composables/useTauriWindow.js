import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow, Window } from '@tauri-apps/api/window';

export function hasTauriRuntime() {
  return (
    typeof window !== 'undefined' &&
    (isTauri() || typeof window.__TAURI_INTERNALS__?.invoke === 'function')
  );
}

export function getTauriWindow() {
  if (!hasTauriRuntime()) {
    return null;
  }

  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

async function resolveTauriWindow() {
  const current = getTauriWindow();

  if (current) {
    return current;
  }

  if (!hasTauriRuntime()) {
    return null;
  }

  try {
    return (await Window.getFocusedWindow()) ?? (await Window.getByLabel('main'));
  } catch {
    return null;
  }
}

export async function closeTauriWindow(options = {}) {
  const appWindow = await resolveTauriWindow();

  if (options.force && hasTauriRuntime()) {
    try {
      await invoke('quit_app');
      return;
    } catch (error) {
      console.error('Rust quit command failed; falling back to window close.', error);
    }
  }

  if (!appWindow) {
    return;
  }

  if (options.force && typeof appWindow.destroy === 'function') {
    try {
      await appWindow.destroy();
      return;
    } catch (error) {
      console.error('Window destroy failed; falling back to window close.', error);
    }
  }

  await appWindow.close();
}

export async function minimizeTauriWindow() {
  await (await resolveTauriWindow())?.minimize();
}

export async function toggleMaximizeTauriWindow() {
  await (await resolveTauriWindow())?.toggleMaximize();
}
