<script setup>
import { onMounted, onUnmounted } from 'vue';
import { useShortcutsModal } from '../composables/useShortcutsModal';

const modal = useShortcutsModal();

const sections = [
  {
    title: 'File Operations',
    shortcuts: [
      { keys: ['F4'],          label: 'Open' },
      { keys: ['F3'],          label: 'Preview' },
      { keys: ['F5'],          label: 'Copy to other pane' },
      { keys: ['⇧', 'F5'],    label: 'Copy here (rename prompt)' },
      { keys: ['F6'],          label: 'Move to other pane' },
      { keys: ['⇧', 'F6'],    label: 'Rename' },
      { keys: ['F7'],          label: 'New folder' },
      { keys: ['F8'],          label: 'Delete' },
    ],
  },
  {
    title: 'Navigation',
    shortcuts: [
      { keys: ['Tab'],         label: 'Switch active pane' },
      { keys: ['⌥', '←'],    label: 'Go back' },
      { keys: ['⌥', '→'],    label: 'Go forward' },
      { keys: ['⌘', '\\'],   label: 'Go to root' },
      { keys: ['⌘', '↑'],    label: 'Go to parent' },
      { keys: ['⌫'],          label: 'Go to parent' },
      { keys: ['F2'],          label: 'Refresh' },
    ],
  },
  {
    title: 'Selection',
    shortcuts: [
      { keys: ['Insert'],      label: 'Toggle item selection' },
      { keys: ['Space'],       label: 'Toggle item selection' },
      { keys: ['⌘', 'A'],    label: 'Select all' },
      { keys: ['Num +'],       label: 'Select all' },
      { keys: ['Num −'],       label: 'Clear selection' },
      { keys: ['Num ×'],       label: 'Invert selection' },
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

function onKeydown(event) {
  if (event.key === 'Escape') {
    modal.hide();
  }
}

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
              <svg width="11" height="11" viewBox="0 0 11 11" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
                <line x1="1" y1="1" x2="10" y2="10" />
                <line x1="10" y1="1" x2="1" y2="10" />
              </svg>
            </button>
          </header>

          <div class="shortcuts-grid">
            <section v-for="section in sections" :key="section.title" class="shortcuts-section">
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
  width: min(720px, calc(100vw - 48px));
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
  background: var(--btn-hover);
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.shortcuts-close:hover {
  background: var(--btn-active-bg);
  color: var(--text);
}

/* ── Grid ─────────────────────────────────────────────────── */
.shortcuts-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px 20px;
  overflow-y: auto;
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
