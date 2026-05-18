<script setup>
import { getTauriWindow } from '../composables/useTauriWindow';

const handles = [
  { direction: 'North', className: 'window-resize-handle--north' },
  { direction: 'South', className: 'window-resize-handle--south' },
  { direction: 'West', className: 'window-resize-handle--west' },
  { direction: 'East', className: 'window-resize-handle--east' },
  { direction: 'NorthWest', className: 'window-resize-handle--north-west' },
  { direction: 'NorthEast', className: 'window-resize-handle--north-east' },
  { direction: 'SouthWest', className: 'window-resize-handle--south-west' },
  { direction: 'SouthEast', className: 'window-resize-handle--south-east' },
];

function startWindowResize(event, direction) {
  if (event.button !== 0) {
    return;
  }

  const appWindow = getTauriWindow();

  if (!appWindow) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();

  appWindow.startResizeDragging(direction).catch((error) => {
    console.error('Unable to start window resize.', error);
  });
}
</script>

<template>
  <div class="window-resize-layer" aria-hidden="true">
    <div
      v-for="handle in handles"
      :key="handle.direction"
      class="window-resize-handle"
      :class="handle.className"
      @mousedown="startWindowResize($event, handle.direction)"
    ></div>
  </div>
</template>

<style scoped>
.window-resize-layer {
  position: absolute;
  inset: 0;
  z-index: 80;
  pointer-events: none;
}

.window-resize-handle {
  position: absolute;
  pointer-events: auto;
}

.window-resize-handle--north {
  top: 0;
  right: 22px;
  left: 22px;
  height: 10px;
  cursor: n-resize;
}

.window-resize-handle--south {
  right: 22px;
  bottom: 0;
  left: 22px;
  height: 14px;
  cursor: s-resize;
}

.window-resize-handle--west {
  top: 22px;
  bottom: 22px;
  left: 0;
  width: 10px;
  cursor: w-resize;
}

.window-resize-handle--east {
  top: 22px;
  right: 0;
  bottom: 22px;
  width: 14px;
  cursor: e-resize;
}

.window-resize-handle--north-west,
.window-resize-handle--north-east,
.window-resize-handle--south-west,
.window-resize-handle--south-east {
  width: 22px;
  height: 22px;
}

.window-resize-handle--north-west {
  top: 0;
  left: 0;
  cursor: nw-resize;
}

.window-resize-handle--north-east {
  top: 0;
  right: 0;
  cursor: ne-resize;
}

.window-resize-handle--south-west {
  bottom: 0;
  left: 0;
  cursor: sw-resize;
}

.window-resize-handle--south-east {
  right: 0;
  bottom: 0;
  cursor: se-resize;
}
</style>
