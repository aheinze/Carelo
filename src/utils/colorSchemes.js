export const DEFAULT_COLOR_SCHEME = 'carelo';

export const COLOR_SCHEME_OPTIONS = Object.freeze([
  {
    value: DEFAULT_COLOR_SCHEME,
    label: 'Carelo',
    description: 'Carelo surfaces with the current blue accent.',
    preview: {
      sidebar: '#1e2220',
      toolbar: '#2c302e',
      pane: '#282d2a',
      accent: '#0a84ff',
    },
    swatches: ['#1e2220', '#2c302e', '#0a84ff'],
  },
  {
    value: 'github',
    label: 'GitHub',
    description: 'GitHub-style neutral surfaces with precise blue accents.',
    preview: {
      sidebar: '#0d1117',
      toolbar: '#161b22',
      pane: '#0d1117',
      accent: '#2f81f7',
    },
    swatches: ['#0d1117', '#2f81f7', '#3fb950'],
  },
  {
    value: 'catppuccin',
    label: 'Catppuccin',
    description: 'Soft pastel surfaces with a calm mauve accent.',
    preview: {
      sidebar: '#181825',
      toolbar: '#1e1e2e',
      pane: '#1e1e2e',
      accent: '#cba6f7',
    },
    swatches: ['#181825', '#cba6f7', '#89b4fa'],
  },
  {
    value: 'dracula',
    label: 'Dracula',
    description: 'High-contrast purple palette with pink and cyan energy.',
    preview: {
      sidebar: '#21222c',
      toolbar: '#282a36',
      pane: '#282a36',
      accent: '#bd93f9',
    },
    swatches: ['#21222c', '#bd93f9', '#ff79c6'],
  },
  {
    value: 'nord',
    label: 'Nord',
    description: 'Cool arctic blue-gray surfaces with restrained accents.',
    preview: {
      sidebar: '#2e3440',
      toolbar: '#3b4252',
      pane: '#2e3440',
      accent: '#88c0d0',
    },
    swatches: ['#2e3440', '#88c0d0', '#a3be8c'],
  },
  {
    value: 'solarized',
    label: 'Solarized',
    description: 'Classic low-contrast palette tuned for long sessions.',
    preview: {
      sidebar: '#002b36',
      toolbar: '#073642',
      pane: '#002b36',
      accent: '#268bd2',
    },
    swatches: ['#002b36', '#268bd2', '#859900'],
  },
  {
    value: 'material',
    label: 'Material',
    description: 'Material-inspired surfaces with a controlled violet accent.',
    preview: {
      sidebar: '#18191c',
      toolbar: '#242529',
      pane: '#1f2023',
      accent: '#bb86fc',
    },
    swatches: ['#18191c', '#bb86fc', '#03dac6'],
  },
  {
    value: 'one-dark-pro',
    label: 'One Dark Pro',
    description: 'Editor-inspired palette with blue and green accents.',
    preview: {
      sidebar: '#21252b',
      toolbar: '#2c313a',
      pane: '#282c34',
      accent: '#61afef',
    },
    swatches: ['#21252b', '#61afef', '#98c379'],
  },
  {
    value: 'tokyo-night',
    label: 'Tokyo Night',
    description: 'Indigo-tinted palette with crisp blue and violet accents.',
    preview: {
      sidebar: '#16161e',
      toolbar: '#24283b',
      pane: '#1a1b26',
      accent: '#7aa2f7',
    },
    swatches: ['#16161e', '#7aa2f7', '#bb9af7'],
  },
]);

const COLOR_SCHEME_VALUES = new Set(COLOR_SCHEME_OPTIONS.map((option) => option.value));

export function normalizeColorScheme(scheme) {
  return COLOR_SCHEME_VALUES.has(scheme) ? scheme : DEFAULT_COLOR_SCHEME;
}

export function applyColorScheme(scheme) {
  if (typeof document === 'undefined') {
    return;
  }

  const normalizedScheme = normalizeColorScheme(scheme);

  if (normalizedScheme === DEFAULT_COLOR_SCHEME) {
    document.documentElement.removeAttribute('data-color-scheme');
  } else {
    document.documentElement.dataset.colorScheme = normalizedScheme;
  }
}
