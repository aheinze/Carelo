<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { useShortcutsModal } from '../composables/useShortcutsModal';

const modal = useShortcutsModal();
const filterInput = ref(null);
const shortcutFilter = ref('');

const sections = [
  {
    title: 'File Operations',
    shortcuts: [
      { keys: ['F4'],          label: 'Edit file' },
      { keys: ['Enter'],       label: 'Open externally' },
      { keys: ['F3'],          label: 'Preview' },
      { keys: ['F5'],          label: 'Copy to other pane' },
      { keys: ['⇧', 'F5'],    label: 'Copy here (rename prompt)' },
      { keys: ['F6'],          label: 'Move to other pane' },
      { keys: ['⇧', 'F6'],    label: 'Rename' },
      { keys: ['⌘', 'M'],    label: 'Rename' },
      { keys: ['F7'],          label: 'New folder' },
      { keys: ['⇧', 'F7'],    label: 'New folder in other pane' },
      { keys: ['F8'],          label: 'Delete' },
      { keys: ['Del'],         label: 'Delete' },
      { keys: ['⇧', 'F10'],   label: 'Context menu' },
    ],
  },
  {
    title: 'Navigation',
    shortcuts: [
      { keys: ['Tab'],         label: 'Switch active pane' },
      { keys: ['⌥', 'F1'],    label: 'Focus left pane' },
      { keys: ['⌥', 'F2'],    label: 'Focus right pane' },
      { keys: ['⌘', '⇧', 'P'], label: 'Command palette' },
      { keys: ['⌘', 'P'],     label: 'Fuzzy search current folder' },
      { keys: ['⌘', '⇧', 'F'], label: 'Search file contents' },
      { keys: ['⌥', '←'],    label: 'Go back' },
      { keys: ['⌥', '→'],    label: 'Go forward' },
      { keys: ['⌘', '['],     label: 'Go back' },
      { keys: ['⌘', ']'],     label: 'Go forward' },
      { keys: ['⌘', '\\'],   label: 'Go to root' },
      { keys: ['⌘', 'PgUp'], label: 'Go to parent' },
      { keys: ['⌘', 'PgDn'], label: 'Open selected folder' },
      { keys: ['⌘', '←'],    label: 'Open focused folder in other pane' },
      { keys: ['⌘', '→'],    label: 'Open focused folder in other pane' },
      { keys: ['⌘', '↑'],    label: 'New tab from focused folder' },
      { keys: ['⌫'],          label: 'Go to parent' },
      { keys: ['F2'],          label: 'Refresh' },
      { keys: ['⌘', 'R'],     label: 'Refresh' },
      { keys: ['⌘', 'F'],     label: 'Focus pane filter' },
      { keys: ['⌘', 'S'],     label: 'Focus pane filter' },
      { keys: ['⌥', 'F7'],    label: 'Focus pane filter' },
    ],
  },
  {
    title: 'Selection',
    shortcuts: [
      { keys: ['↑'],           label: 'Move selection up' },
      { keys: ['↓'],           label: 'Move selection down' },
      { keys: ['Home'],        label: 'First item' },
      { keys: ['End'],         label: 'Last item' },
      { keys: ['PgUp'],        label: 'Page up' },
      { keys: ['PgDn'],        label: 'Page down' },
      { keys: ['Insert'],      label: 'Toggle item selection' },
      { keys: ['Space'],       label: 'Toggle item selection' },
      { keys: ['⌘', 'A'],    label: 'Select all' },
      { keys: ['Num +'],       label: 'Select all' },
      { keys: ['⌥', 'Num +'], label: 'Select same extension' },
      { keys: ['Num −'],       label: 'Clear selection' },
      { keys: ['⌥', 'Num −'], label: 'Clear same extension' },
      { keys: ['Num /'],       label: 'Clear selection' },
      { keys: ['Num ×'],       label: 'Invert selection' },
    ],
  },
  {
    title: 'Clipboard & Paths',
    shortcuts: [
      { keys: ['⌘', 'C'],     label: 'Copy files' },
      { keys: ['⌘', 'X'],     label: 'Cut files' },
      { keys: ['⌘', 'V'],     label: 'Paste files' },
      { keys: ['⌘', 'Insert'], label: 'Copy files' },
      { keys: ['⇧', 'Insert'], label: 'Paste files' },
      { keys: ['⌘', 'Enter'], label: 'Copy focused name' },
      { keys: ['⌘', '⇧', 'Enter'], label: 'Copy focused path' },
    ],
  },
  {
    title: 'Tabs & Layout',
    shortcuts: [
      { keys: ['⌘', 'Tab'],   label: 'Next tab' },
      { keys: ['⌘', '⇧', 'Tab'], label: 'Previous tab' },
      { keys: ['⌘', 'T'],     label: 'New tab' },
      { keys: ['⌘', 'W'],     label: 'Close tab' },
      { keys: ['⌘', 'U'],     label: 'Swap panes' },
      { keys: ['⌘', 'B'],     label: 'Toggle sidebar' },
      { keys: ['⌘', 'I'],     label: 'Toggle preview panel' },
      { keys: ['⌘', 'Q'],     label: 'Toggle preview panel' },
      { keys: ['⌘', '`'],     label: 'Toggle terminal' },
      { keys: ['⌘', ','],     label: 'Settings' },
      { keys: ['⌥', 'Enter'], label: 'Show preview panel' },
      { keys: ['⌘', '.'],     label: 'Toggle hidden files' },
    ],
  },
  {
    title: 'View & Sort',
    shortcuts: [
      { keys: ['⌘', 'F1'],   label: 'Grid view' },
      { keys: ['⌘', 'F2'],   label: 'List view' },
      { keys: ['⌘', 'F3'],   label: 'Sort by name' },
      { keys: ['⌘', 'F4'],   label: 'Sort by extension' },
      { keys: ['⌘', 'F5'],   label: 'Sort by date modified' },
      { keys: ['⌘', 'F6'],   label: 'Sort by size' },
      { keys: ['⌘', 'F7'],   label: 'No sorting' },
    ],
  },
];

const keyAliases = {
  '⌘': 'command cmd meta',
  '⇧': 'shift',
  '⌥': 'option alt',
  '←': 'left arrow back',
  '→': 'right arrow forward',
  '↑': 'up arrow',
  '↓': 'down arrow',
  '⌫': 'backspace parent up',
  Del: 'delete remove',
  Enter: 'return',
  Space: 'spacebar',
  Insert: 'ins',
  Home: 'start first',
  End: 'last',
  PgUp: 'page up',
  PgDn: 'page down',
  'Num +': 'numpad plus add',
  'Num −': 'numpad minus subtract',
  'Num /': 'numpad slash divide',
  'Num ×': 'numpad multiply',
};

const normalizedShortcutFilter = computed(() => normalizeShortcutText(shortcutFilter.value));
const filteredSections = computed(() => {
  const query = normalizedShortcutFilter.value;

  if (!query) {
    return sections;
  }

  return sections
    .map((section) => {
      const sectionMatches = matchesShortcutQuery(section.title, query);

      return {
        ...section,
        shortcuts: section.shortcuts.filter((shortcut) => (
          sectionMatches || matchesShortcutQuery(shortcutSearchText(section, shortcut), query)
        )),
      };
    })
    .filter((section) => section.shortcuts.length > 0);
});
const visibleShortcutCount = computed(() => (
  filteredSections.value.reduce((total, section) => total + section.shortcuts.length, 0)
));

function normalizeShortcutText(value) {
  return String(value || '').toLowerCase().replace(/\s+/g, ' ').trim();
}

function matchesShortcutQuery(value, query) {
  if (!query) {
    return true;
  }

  const haystack = normalizeShortcutText(value);

  return query.split(' ').every((term) => haystack.includes(term));
}

function shortcutSearchText(section, shortcut) {
  const keyText = shortcut.keys.join(' ');
  const aliasText = shortcut.keys.map((key) => keyAliases[key] || '').join(' ');

  return `${section.title} ${shortcut.label} ${keyText} ${aliasText}`;
}

function focusFilter() {
  nextTick(() => {
    filterInput.value?.focus?.({ preventScroll: true });
  });
}

function clearFilter() {
  shortcutFilter.value = '';
  focusFilter();
}

function handleFilterEscape() {
  if (shortcutFilter.value) {
    clearFilter();
    return;
  }

  modal.hide();
}

function onKeydown(event) {
  if (event.key === 'Escape') {
    modal.hide();
  }
}

watch(() => modal.visible.value, (visible) => {
  if (visible) {
    shortcutFilter.value = '';
    focusFilter();
  }
});

onMounted(() => window.addEventListener('keydown', onKeydown));
onUnmounted(() => window.removeEventListener('keydown', onKeydown));
</script>

<template>
  <Teleport to="body">
    <Transition name="shortcuts-fade">
      <div
        v-if="modal.visible.value"
        class="shortcuts-overlay"
        role="dialog"
        aria-modal="true"
        aria-label="Commander shortcuts"
        @pointerdown.self="modal.hide()"
      >
        <div class="shortcuts-panel">

          <header class="shortcuts-header">
            <div class="shortcuts-title-group">
              <span class="shortcuts-badge">F1</span>
              <h2>Commander Shortcuts</h2>
            </div>
            <button type="button" class="shortcuts-close" aria-label="Close" @click="modal.hide()">
              <AppIcon name="x" :size="14" :stroke-width="2" />
            </button>
          </header>

          <label class="shortcuts-filter">
            <AppIcon name="search" :size="15" :stroke-width="1.9" />
            <input
              ref="filterInput"
              v-model="shortcutFilter"
              type="search"
              autocomplete="off"
              spellcheck="false"
              placeholder="Filter shortcuts..."
              aria-label="Filter shortcuts"
              @keydown.escape.prevent.stop="handleFilterEscape"
            />
            <span v-if="shortcutFilter" class="shortcuts-filter-count">
              {{ visibleShortcutCount }}
            </span>
            <button
              v-if="shortcutFilter"
              type="button"
              class="shortcuts-filter-clear"
              aria-label="Clear shortcut filter"
              @click="clearFilter"
            >
              <AppIcon name="x" :size="13" :stroke-width="2.2" />
            </button>
          </label>

          <div v-if="filteredSections.length > 0" class="shortcuts-grid">
            <section v-for="section in filteredSections" :key="section.title" class="shortcuts-section">
              <h3>{{ section.title }}</h3>
              <ul>
                <li v-for="shortcut in section.shortcuts" :key="shortcut.label + shortcut.keys.join()">
                  <span class="shortcut-keys">
                    <kbd v-for="(key, i) in shortcut.keys" :key="i">{{ key }}</kbd>
                  </span>
                  <span class="shortcut-label">{{ shortcut.label }}</span>
                </li>
              </ul>
            </section>
          </div>

          <div v-else class="shortcuts-empty">
            <AppIcon name="search" :size="24" :stroke-width="1.5" />
            <span>No shortcuts found</span>
          </div>

        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
/* ── Overlay ──────────────────────────────────────────────── */
.shortcuts-overlay {
  position: fixed;
  z-index: 5000;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 32px;
  background: var(--overlay-bg);
  backdrop-filter: blur(22px) saturate(1.1);
  -webkit-backdrop-filter: blur(22px) saturate(1.1);
}

/* ── Panel ────────────────────────────────────────────────── */
.shortcuts-panel {
  display: flex;
  flex-direction: column;
  gap: 20px;
  width: min(980px, calc(100vw - 48px));
  max-height: calc(100vh - 80px);
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 18px;
  padding: 22px 24px 24px;
  background: var(--modal-bg);
  box-shadow: var(--shadow-overlay);
}

/* ── Header ───────────────────────────────────────────────── */
.shortcuts-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
}

.shortcuts-title-group {
  display: flex;
  align-items: center;
  gap: 11px;
}

.shortcuts-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  height: 22px;
  padding: 0 8px;
  border: 1px solid rgb(var(--accent-rgb) / 0.4);
  border-radius: 6px;
  background: var(--accent-dim);
  color: var(--accent);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.04em;
}

.shortcuts-header h2 {
  margin: 0;
  color: var(--text);
  font-size: 15px;
  font-weight: 700;
  letter-spacing: -0.01em;
}

.shortcuts-close {
  display: grid;
  width: 26px;
  height: 26px;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.shortcuts-close:hover {
  background: var(--btn-hover);
  color: var(--text);
}

/* ── Grid ─────────────────────────────────────────────────── */
.shortcuts-filter {
  display: grid;
  grid-template-columns: 18px minmax(0, 1fr) auto auto;
  align-items: center;
  gap: 9px;
  flex-shrink: 0;
  min-height: 38px;
  padding: 0 8px 0 12px;
  border: 1px solid var(--input-border);
  border-radius: 10px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text-faint);
}

.shortcuts-filter:focus-within {
  border-color: var(--accent-border);
  color: var(--text-muted);
  box-shadow:
    var(--input-shadow),
    var(--accent-focus-ring);
}

.shortcuts-filter input {
  width: 100%;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--text);
  font: inherit;
  font-size: 13px;
  font-weight: 520;
  letter-spacing: 0;
}

.shortcuts-filter input::placeholder {
  color: var(--text-faint);
}

.shortcuts-filter input::-webkit-search-cancel-button {
  appearance: none;
}

.shortcuts-filter-count {
  min-width: 24px;
  padding: 3px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text) 7%, transparent);
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 650;
  text-align: center;
}

.shortcuts-filter-clear {
  display: grid;
  place-items: center;
  width: 24px;
  height: 24px;
  border-radius: 7px;
  background: transparent;
  color: var(--text-faint);
}

.shortcuts-filter-clear:hover,
.shortcuts-filter-clear:focus-visible {
  background: var(--btn-hover);
  color: var(--text);
  outline: 0;
}

.shortcuts-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 6px 20px;
  overflow-y: auto;
}

@media (max-width: 860px) {
  .shortcuts-grid {
    grid-template-columns: 1fr 1fr;
  }
}

@media (max-width: 620px) {
  .shortcuts-grid {
    grid-template-columns: 1fr;
  }
}

/* ── Section ──────────────────────────────────────────────── */
.shortcuts-section {
  padding: 14px 16px;
  border-radius: 12px;
  background: color-mix(in srgb, var(--text) 3.5%, transparent);
  border: 1px solid var(--hairline);
}

.shortcuts-section h3 {
  margin: 0 0 10px;
  color: var(--accent);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  opacity: 0.8;
}

.shortcuts-section ul {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

/* ── Shortcut row ─────────────────────────────────────────── */
li {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.shortcut-keys {
  display: flex;
  align-items: center;
  gap: 3px;
  flex-shrink: 0;
}

kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 22px;
  height: 20px;
  padding: 0 6px;
  border: 1px solid var(--control-border);
  border-radius: 5px;
  background: var(--control-bg);
  box-shadow: var(--control-inset);
  color: var(--text);
  font-family: inherit;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0;
  white-space: nowrap;
}

.shortcut-label {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 480;
  text-align: right;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.shortcuts-empty {
  display: grid;
  place-items: center;
  align-content: center;
  gap: 10px;
  min-height: 220px;
  border: 1px solid var(--hairline);
  border-radius: 12px;
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
  color: var(--text-faint);
  font-size: 13px;
  font-weight: 600;
}

/* ── Animation ────────────────────────────────────────────── */
.shortcuts-fade-enter-active {
  transition: opacity 180ms ease;
}
.shortcuts-fade-leave-active {
  transition: opacity 140ms ease;
}
.shortcuts-fade-enter-active .shortcuts-panel {
  transition: transform 220ms cubic-bezier(0.2, 0, 0, 1), opacity 180ms ease;
}
.shortcuts-fade-leave-active .shortcuts-panel {
  transition: transform 140ms ease, opacity 120ms ease;
}
.shortcuts-fade-enter-from,
.shortcuts-fade-leave-to {
  opacity: 0;
}
.shortcuts-fade-enter-from .shortcuts-panel,
.shortcuts-fade-leave-to .shortcuts-panel {
  opacity: 0;
  transform: scale(0.97) translateY(8px);
}
</style>
