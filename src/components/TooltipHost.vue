<script setup>
import { onUpdated, ref, watch } from 'vue';
import { tooltipState } from '../composables/useTooltipState';

const GAP = 10;
const EDGE = 8;

const tooltipEl = ref(null);
const resolvedLeft = ref(0);
const resolvedTop = ref(0);
const positioned = ref(false);

function resolvePosition() {
  const el = tooltipEl.value;
  if (!el) return;

  const rect = el.getBoundingClientRect();
  const vw = window.innerWidth;

  // Center horizontally, clamped to viewport edges
  const halfW = rect.width / 2;
  let left = tooltipState.x;
  left = Math.max(EDGE + halfW, Math.min(left, vw - EDGE - halfW));

  // Prefer above; flip below if not enough vertical clearance
  let top;
  if (tooltipState.y - rect.height - GAP >= EDGE) {
    top = tooltipState.y - rect.height - GAP;
  } else {
    top = tooltipState.targetBottom + GAP;
  }

  resolvedLeft.value = left;
  resolvedTop.value = top;
  positioned.value = true;
}

// Reset positioned state before each re-render so we re-measure
watch(
  () => [
    tooltipState.visible,
    tooltipState.x,
    tooltipState.y,
    tooltipState.text,
    tooltipState.description,
    tooltipState.shortcut,
  ],
  () => { positioned.value = false; },
  { flush: 'pre' },
);

// After DOM updates, measure and position if not yet done
onUpdated(() => {
  if (tooltipState.visible && tooltipEl.value && !positioned.value) {
    resolvePosition();
  }
});

</script>

<template>
  <Teleport to="body">
    <div
      v-if="tooltipState.visible && tooltipState.text"
      ref="tooltipEl"
      class="app-tooltip"
      :class="{
        'app-tooltip--ready': positioned,
        'app-tooltip--rich': tooltipState.description,
        'app-tooltip--shortcut': tooltipState.shortcut,
      }"
      :style="{
        left: `${positioned ? resolvedLeft : tooltipState.x}px`,
        top: positioned ? `${resolvedTop}px` : '-9999px',
      }"
    >
      <span class="app-tooltip-row">
        <span class="app-tooltip-text">{{ tooltipState.text }}</span>
        <kbd v-if="tooltipState.shortcut" class="kbd app-tooltip-key">{{ tooltipState.shortcut }}</kbd>
      </span>
      <span v-if="tooltipState.description" class="app-tooltip-description">{{ tooltipState.description }}</span>
    </div>
  </Teleport>
</template>

<style>
.app-tooltip {
  position: fixed;
  z-index: 9999;
  pointer-events: none;
  transform: translateX(-50%);
  opacity: 0;
  padding: 5px 9px 6px;
  border-radius: 7px;
  background: var(--popover-bg);
  border: 1px solid var(--control-border);
  color: var(--text);
  font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", system-ui, sans-serif;
  font-size: 12px;
  font-weight: 520;
  line-height: 1;
  white-space: nowrap;
  box-shadow: var(--shadow-overlay);
}

.app-tooltip--rich {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 6px 10px 7px;
}

.app-tooltip-row {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}

.app-tooltip-text {
  color: var(--text);
  font-size: 12px;
  font-weight: 580;
  line-height: 1;
}

.app-tooltip-key {
  height: 16px;
  min-width: 18px;
  padding: 0 5px;
  font-size: 10px;
  color: var(--text-muted);
}

.app-tooltip-description {
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 460;
  line-height: 1.3;
}

.app-tooltip--ready {
  animation: tooltip-in 110ms cubic-bezier(0.2, 0, 0, 1) forwards;
}

@keyframes tooltip-in {
  from {
    opacity: 0;
    translate: 0 4px;
  }
  to {
    opacity: 1;
    translate: 0 0;
  }
}
</style>
