<script setup>
import { computed } from 'vue';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { saveCurrentWindowDimensions } from '../composables/useWindowDimensions';
import {
  closeTauriWindow,
  getTauriWindow,
  minimizeTauriWindow,
  toggleMaximizeTauriWindow,
} from '../composables/useTauriWindow';

const store = useFileManagerStore();

const activePath = computed(() => store.activePane?.currentPath || '~');

function startDragging(event) {
  if (event.button !== 0 || event.detail > 1) {
    return;
  }

  event.preventDefault();
  getTauriWindow()?.startDragging().catch((error) => {
    console.error('Unable to start window drag.', error);
  });
}

function minimizeWindow(event) {
  event?.stopPropagation();
  minimizeTauriWindow().catch(() => {});
}

function toggleMaximizeWindow(event) {
  event?.stopPropagation();
  toggleMaximizeTauriWindow().catch(() => {});
}

function closeWindow(event) {
  event?.stopPropagation();
  saveCurrentWindowDimensions();
  closeTauriWindow({ force: true }).catch(() => {});
}
</script>

<template>
  <header
    class="window-bar"
    aria-label="Window controls"
    @mousedown="startDragging"
    @dblclick="toggleMaximizeWindow"
  >
    <div class="window-controls" aria-label="Window actions">
      <button
        type="button"
        class="window-control window-control--close"
        aria-label="Close window"
        @pointerdown.stop
        @mousedown.stop
        @dblclick.stop
        @click.stop.prevent="closeWindow"
      >
        <span aria-hidden="true"></span>
      </button>
      <button
        type="button"
        class="window-control window-control--minimize"
        aria-label="Minimize window"
        @pointerdown.stop
        @mousedown.stop
        @dblclick.stop
        @click.stop.prevent="minimizeWindow"
      >
        <span aria-hidden="true"></span>
      </button>
      <button
        type="button"
        class="window-control window-control--zoom"
        aria-label="Zoom window"
        @pointerdown.stop
        @mousedown.stop
        @dblclick.stop
        @click.stop.prevent="toggleMaximizeWindow"
      >
        <span aria-hidden="true"></span>
      </button>
    </div>

    <div
      class="window-drag-region"
      data-tauri-drag-region
    >
      <strong>Carelo</strong>
      <span>{{ activePath }}</span>
    </div>

  </header>
</template>

<style scoped>
.window-bar {
  display: grid;
  grid-column: 1 / -1;
  grid-template-columns: 92px minmax(0, 1fr);
  align-items: center;
  min-width: 0;
  min-height: 36px;
  border-radius: 10px;
  background: var(--chrome-bar-bg);
  box-shadow:
    inset 0 1px 0 var(--hairline),
    inset 0 -1px 0 var(--separator);
  user-select: none;
}

/* ── Traffic lights ───────────────────────────────────────── */
.window-controls {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-left: 14px;
}

.window-control {
  position: relative;
  display: grid;
  width: 12px;
  height: 12px;
  place-items: center;
  border-radius: 50%;
  padding: 0;
  box-shadow:
    inset 0 0 0 0.5px rgb(0 0 0 / 0.35),
    0 1px 2px rgb(0 0 0 / 0.3);
}

.window-control span {
  width: 6px;
  height: 6px;
  opacity: 0;
  transition: opacity 90ms ease;
}

.window-controls:hover .window-control span {
  opacity: 0.75;
}

.window-control--close { background: var(--traffic-close); }

.window-control--close span::before,
.window-control--close span::after {
  position: absolute;
  top: 5.5px;
  left: 3.4px;
  width: 5.5px;
  height: 1px;
  border-radius: 1px;
  background: rgb(80 0 0 / 0.75);
  content: "";
}

.window-control--close span::before { transform: rotate(45deg); }
.window-control--close span::after  { transform: rotate(-45deg); }

.window-control--minimize { background: var(--traffic-minimize); }

.window-control--minimize span::before {
  position: absolute;
  top: 5.6px;
  left: 3.4px;
  width: 5.4px;
  height: 1.2px;
  border-radius: 1px;
  background: rgb(88 58 0 / 0.75);
  content: "";
}

.window-control--zoom { background: var(--traffic-zoom); }

.window-control--zoom span::before {
  position: absolute;
  top: 3.6px;
  left: 3.8px;
  width: 4.7px;
  height: 4.7px;
  border: 1px solid rgb(0 70 14 / 0.68);
  border-radius: 1px;
  content: "";
}

/* ── Center drag region ───────────────────────────────────── */
.window-drag-region {
  display: flex;
  min-width: 0;
  height: 100%;
  align-items: center;
  justify-content: center;
  gap: 0;
  cursor: default;
}

.window-drag-region strong {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 660;
  letter-spacing: 0.01em;
  flex-shrink: 0;
}

.window-drag-region strong::after {
  content: "·";
  margin: 0 7px;
  color: var(--text-faint);
  font-weight: 400;
}

.window-drag-region span {
  overflow: hidden;
  max-width: min(48vw, 560px);
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
  letter-spacing: 0.01em;
}

</style>
