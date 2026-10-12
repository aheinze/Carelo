<script setup>
import { computed } from 'vue';
import {
  closeTauriWindow,
  minimizeTauriWindow,
  toggleMaximizeTauriWindow,
} from '../composables/useTauriWindow';

const props = defineProps({
  // 'left' renders the macOS order, 'right' the GNOME/Windows one.
  position: {
    type: String,
    default: 'left',
  },
});

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
  closeTauriWindow({ force: true }).catch(() => {});
}

const controls = computed(() => {
  const close = { key: 'close', label: 'Close window', action: closeWindow };
  const minimize = { key: 'minimize', label: 'Minimize window', action: minimizeWindow };
  const zoom = { key: 'zoom', label: 'Zoom window', action: toggleMaximizeWindow };

  return props.position === 'right' ? [minimize, zoom, close] : [close, minimize, zoom];
});
</script>

<template>
  <div class="window-controls" aria-label="Window actions" @mousedown.stop @dblclick.stop>
    <button
      v-for="control in controls"
      :key="control.key"
      type="button"
      class="window-control"
      :class="`window-control--${control.key}`"
      :aria-label="control.label"
      @pointerdown.stop
      @mousedown.stop
      @dblclick.stop
      @click.stop.prevent="control.action"
    >
      <span aria-hidden="true"></span>
    </button>
  </div>
</template>

<style scoped>
/* ── Traffic lights ───────────────────────────────────────── */
.window-controls {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: 0 0 auto;
}

.window-control {
  position: relative;
  display: grid;
  width: 13px;
  height: 13px;
  place-items: center;
  border-radius: 50%;
  padding: 0;
  box-shadow:
    inset 0 0 0 0.5px rgb(0 0 0 / 0.35),
    0 1px 2px rgb(0 0 0 / 0.25);
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
.window-control--minimize { background: var(--traffic-minimize); }
.window-control--zoom { background: var(--traffic-zoom); }

.window-control--close span::before,
.window-control--close span::after {
  position: absolute;
  top: 6px;
  left: 3.7px;
  width: 5.7px;
  height: 1px;
  border-radius: 1px;
  background: rgb(80 0 0 / 0.75);
  content: "";
}

.window-control--close span::before { transform: rotate(45deg); }
.window-control--close span::after { transform: rotate(-45deg); }

.window-control--minimize span::before {
  position: absolute;
  top: 6px;
  left: 3.8px;
  width: 5.7px;
  height: 1.2px;
  border-radius: 1px;
  background: rgb(88 58 0 / 0.75);
  content: "";
}

.window-control--zoom span::before {
  position: absolute;
  top: 3.9px;
  left: 4px;
  width: 4.8px;
  height: 4.8px;
  border: 1px solid rgb(0 70 14 / 0.68);
  border-radius: 1px;
  content: "";
}
</style>
