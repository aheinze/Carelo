const SETTINGS_KEY = 'carelo.phase1.settings';

export function loadUiSettings() {
  try {
    const raw = window.localStorage.getItem(SETTINGS_KEY);
    return raw ? JSON.parse(raw) : {};
  } catch {
    return {};
  }
}

export function saveUiSettings(settings) {
  try {
    window.localStorage.setItem(SETTINGS_KEY, JSON.stringify({
      ...loadUiSettings(),
      ...settings,
    }));
  } catch {
    // Settings persistence is best-effort in Phase 1.
  }
}
