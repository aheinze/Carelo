<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';

const props = defineProps({
  tab: {
    type: Object,
    required: true,
  },
  title: {
    type: String,
    required: true,
  },
  position: {
    type: Object,
    required: true,
  },
  canClose: {
    type: Boolean,
    default: true,
  },
  canCloseOthers: {
    type: Boolean,
    default: true,
  },
  isActive: {
    type: Boolean,
    default: false,
  },
});

const emit = defineEmits(['action', 'close']);

const menuRef = ref(null);
const menuStyle = ref({
  left: `${props.position.x}px`,
  top: `${props.position.y}px`,
  maxHeight: 'calc(100vh - 16px)',
});
const menuGroups = computed(() => [
  [
    {
      action: 'copyPath',
      icon: 'copy',
      label: 'Copy Path',
    },
    {
      action: 'duplicate',
      icon: 'plus',
      label: 'Duplicate Tab',
    },
  ],
  [
    {
      action: 'openInOtherPane',
      icon: 'open-other-pane',
      label: 'Open in Other Pane',
    },
    {
      action: 'moveToOtherPane',
      icon: 'chevron-right',
      label: 'Move to Other Pane',
    },
  ],
  [
    {
      action: 'close',
      icon: 'x',
      label: 'Close Tab',
      shortcut: props.isActive && props.canClose ? 'Ctrl W' : undefined,
      disabled: !props.canClose,
    },
    {
      action: 'closeOthers',
      icon: 'x',
      label: 'Close Other Tabs',
      disabled: !props.canCloseOthers,
    },
  ],
]
  .map((group) => group.filter((item) => !item.disabled))
  .filter((group) => group.length > 0));

function updatePosition() {
  nextTick(() => {
    const menu = menuRef.value;

    if (!menu) {
      return;
    }

    const margin = 8;
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const rect = menu.getBoundingClientRect();
    const maxHeight = Math.max(1, viewportHeight - margin * 2);
    const menuHeight = Math.min(menu.scrollHeight, maxHeight);
    const wouldOverflowRight = props.position.x + rect.width + margin > viewportWidth;
    const wouldOverflowBottom = props.position.y + menuHeight + margin > viewportHeight;
    const canFitLeft = props.position.x - rect.width >= margin;
    const canFitAbove = props.position.y - menuHeight >= margin;
    const preferredLeft = wouldOverflowRight && canFitLeft
      ? props.position.x - rect.width
      : props.position.x;
    const preferredTop = wouldOverflowBottom && canFitAbove
      ? props.position.y - menuHeight
      : props.position.y;
    const left = Math.min(
      Math.max(margin, preferredLeft),
      Math.max(margin, viewportWidth - rect.width - margin),
    );
    const top = Math.min(
      Math.max(margin, preferredTop),
      Math.max(margin, viewportHeight - menuHeight - margin),
    );

    menuStyle.value = {
      left: `${left}px`,
      top: `${top}px`,
      maxHeight: `${maxHeight}px`,
    };
  });
}

function emitAction(action) {
  emit('action', action);
}

function handleWindowPointerDown(event) {
  if (!menuRef.value?.contains(event.target)) {
    emit('close');
  }
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault();
    emit('close');
  }
}

watch(() => props.position, updatePosition, { deep: true });

onMounted(() => {
  updatePosition();
  window.addEventListener('pointerdown', handleWindowPointerDown);
  window.addEventListener('keydown', handleKeydown);
  window.addEventListener('resize', updatePosition);
});

onUnmounted(() => {
  window.removeEventListener('pointerdown', handleWindowPointerDown);
  window.removeEventListener('keydown', handleKeydown);
  window.removeEventListener('resize', updatePosition);
});
</script>

<template>
  <Teleport to="body">
    <div
      ref="menuRef"
      class="tab-context-menu"
      :style="menuStyle"
      role="menu"
      :aria-label="`${title} tab context menu`"
      @contextmenu.prevent
    >
      <div class="tab-context-menu-title">
        <span>{{ title }}</span>
        <small>{{ tab.currentPath }}</small>
      </div>

      <template v-for="(group, groupIndex) in menuGroups" :key="groupIndex">
        <div v-if="groupIndex > 0" class="tab-context-menu-separator"></div>

        <button
          v-for="item in group"
          :key="item.action"
          type="button"
          role="menuitem"
          class="tab-context-menu-item"
          :class="{ 'tab-context-menu-item--shortcut': item.shortcut }"
          :disabled="item.disabled"
          @click="emitAction(item.action)"
        >
          <AppIcon :name="item.icon" :size="16" />
          <span class="tab-context-menu-item-label">{{ item.label }}</span>
          <kbd v-if="item.shortcut" class="kbd tab-context-menu-shortcut">{{ item.shortcut }}</kbd>
        </button>
      </template>
    </div>
  </Teleport>
</template>

<style scoped>
.tab-context-menu {
  position: fixed;
  z-index: 2000;
  min-width: 224px;
  overflow-x: hidden;
  overflow-y: auto;
  overscroll-behavior: contain;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  padding: 5px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  animation: tab-context-appear 130ms cubic-bezier(0.2, 0, 0, 1) both;
  transform-origin: top left;
  scrollbar-width: thin;
  scrollbar-color: var(--control-border) transparent;
}

.tab-context-menu::-webkit-scrollbar {
  width: 9px;
}

.tab-context-menu::-webkit-scrollbar-track {
  background: transparent;
}

.tab-context-menu::-webkit-scrollbar-thumb {
  border: 2px solid transparent;
  border-radius: 999px;
  background: var(--control-border);
  background-clip: padding-box;
}

@keyframes tab-context-appear {
  from {
    opacity: 0;
    transform: scale(0.93);
  }

  to {
    opacity: 1;
    transform: scale(1);
  }
}

.tab-context-menu-title {
  display: grid;
  gap: 1px;
  padding: 5px 10px 8px;
  border-bottom: 1px solid var(--hairline);
  margin-bottom: 4px;
}

.tab-context-menu-title span,
.tab-context-menu-title small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tab-context-menu-title span {
  font-size: 12.5px;
  font-weight: 660;
}

.tab-context-menu-title small {
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 620;
  letter-spacing: 0;
}

.tab-context-menu-item {
  display: grid;
  width: 100%;
  grid-template-columns: 18px minmax(0, 1fr);
  align-items: center;
  gap: 9px;
  min-height: 26px;
  border-radius: 8px;
  padding: 0 10px;
  background: transparent;
  color: var(--text);
  font-size: 13px;
  font-weight: 500;
  text-align: left;
  transition: none;
}

.tab-context-menu-item--shortcut {
  grid-template-columns: 18px minmax(0, 1fr) auto;
}

.tab-context-menu-item-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tab-context-menu-shortcut {
  justify-self: end;
  min-width: max-content;
  pointer-events: none;
}

.tab-context-menu-item:hover:not(:disabled),
.tab-context-menu-item:focus-visible {
  background: var(--btn-primary-bg);
  color: #fff;
  outline: 0;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.tab-context-menu-item:hover:not(:disabled) .tab-context-menu-shortcut,
.tab-context-menu-item:focus-visible .tab-context-menu-shortcut {
  border-color: rgb(255 255 255 / 0.28);
  background: rgb(255 255 255 / 0.14);
  box-shadow: none;
  color: rgb(255 255 255 / 0.82);
}

.tab-context-menu-item:disabled {
  cursor: default;
  color: var(--text-faint);
  opacity: 0.45;
}

.tab-context-menu-separator {
  height: 1px;
  margin: 4px 2px;
  background: var(--hairline);
}
</style>
