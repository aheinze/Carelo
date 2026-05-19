<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';

const props = defineProps({
  entry: {
    type: Object,
    default: null,
  },
  position: {
    type: Object,
    required: true,
  },
  canTransfer: {
    type: Boolean,
    default: true,
  },
  canArchive: {
    type: Boolean,
    default: true,
  },
  canUnarchive: {
    type: Boolean,
    default: false,
  },
  canOpenWith: {
    type: Boolean,
    default: true,
  },
  canModify: {
    type: Boolean,
    default: true,
  },
  canMove: {
    type: Boolean,
    default: true,
  },
});

const emit = defineEmits(['action', 'close']);

const menuRef = ref(null);
const menuStyle = ref({
  left: `${props.position.x}px`,
  top: `${props.position.y}px`,
  maxHeight: 'calc(100vh - 16px)',
});

const canOpenInNewTab = computed(() => props.entry?.kind === 'directory');
const itemType = computed(() => (props.entry?.kind === 'directory' ? 'Folder' : 'File'));

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
      v-if="entry"
      ref="menuRef"
      class="context-menu"
      :style="menuStyle"
      role="menu"
      :aria-label="`${entry.name} context menu`"
      @contextmenu.prevent
    >
      <div class="context-menu-title">
        <span>{{ entry.name }}</span>
        <small>{{ itemType }}</small>
      </div>

      <button type="button" role="menuitem" class="context-menu-item" @click="emitAction('open')">
        <AppIcon :name="entry.kind === 'directory' ? 'folder' : 'file'" :size="16" />
        <span>Open</span>
      </button>
      <button
        type="button"
        role="menuitem"
        class="context-menu-item"
        :disabled="!canOpenWith"
        @click="emitAction('openWith')"
      >
        <AppIcon name="app" :size="16" />
        <span>Open With…</span>
      </button>
      <button
        type="button"
        role="menuitem"
        class="context-menu-item"
        :disabled="!canOpenInNewTab"
        @click="emitAction('openInNewTab')"
      >
        <AppIcon name="plus" :size="16" />
        <span>Open in New Tab</span>
      </button>
      <button type="button" role="menuitem" class="context-menu-item" @click="emitAction('reveal')">
        <AppIcon name="folder" :size="16" />
        <span>Reveal in File Manager</span>
      </button>

      <div class="context-menu-separator"></div>

      <button type="button" role="menuitem" class="context-menu-item" @click="emitAction('copyPath')">
        <AppIcon name="copy" :size="16" />
        <span>Copy Path</span>
      </button>
      <button
        type="button"
        role="menuitem"
        class="context-menu-item"
        :disabled="!canModify"
        @click="emitAction('rename')"
      >
        <AppIcon name="file" :size="16" />
        <span>Rename</span>
      </button>

      <div class="context-menu-separator"></div>

      <button
        type="button"
        role="menuitem"
        class="context-menu-item"
        :disabled="!canArchive"
        @click="emitAction('archive')"
      >
        <AppIcon name="archive" :size="16" />
        <span>Create Archive</span>
      </button>
      <button
        type="button"
        role="menuitem"
        class="context-menu-item"
        :disabled="!canUnarchive"
        @click="emitAction('unarchive')"
      >
        <AppIcon name="extract" :size="16" />
        <span>Extract Zip Archive</span>
      </button>

      <div class="context-menu-separator"></div>

      <button
        type="button"
        role="menuitem"
        class="context-menu-item"
        :disabled="!canTransfer"
        @click="emitAction('copyToOtherPane')"
      >
        <AppIcon name="copy" :size="16" />
        <span>Copy to Other Pane</span>
      </button>
      <button
        type="button"
        role="menuitem"
        class="context-menu-item"
        :disabled="!canMove"
        @click="emitAction('moveToOtherPane')"
      >
        <AppIcon name="chevron-right" :size="16" />
        <span>Move to Other Pane</span>
      </button>

      <div class="context-menu-separator"></div>

      <button
        type="button"
        role="menuitem"
        class="context-menu-item context-menu-item--danger"
        :disabled="!canModify"
        @click="emitAction('delete')"
      >
        <AppIcon name="trash" :size="16" />
        <span>Delete</span>
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.context-menu {
  position: fixed;
  z-index: 2000;
  min-width: 224px;
  overflow-x: hidden;
  overflow-y: auto;
  overscroll-behavior: contain;
  border: 1px solid var(--control-border);
  border-radius: 13px;
  padding: 5px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  animation: ctx-appear 130ms cubic-bezier(0.2, 0, 0, 1) both;
  transform-origin: top left;
  scrollbar-width: thin;
  scrollbar-color: var(--control-border) transparent;
}

.context-menu::-webkit-scrollbar {
  width: 9px;
}

.context-menu::-webkit-scrollbar-track {
  background: transparent;
}

.context-menu::-webkit-scrollbar-thumb {
  border: 2px solid transparent;
  border-radius: 999px;
  background: var(--control-border);
  background-clip: padding-box;
}

@keyframes ctx-appear {
  from {
    opacity: 0;
    transform: scale(0.93);
  }
  to {
    opacity: 1;
    transform: scale(1);
  }
}

/* ── Title header ─────────────────────────────────────────── */
.context-menu-title {
  display: grid;
  gap: 1px;
  padding: 5px 10px 8px;
  border-bottom: 1px solid var(--hairline);
  margin-bottom: 4px;
}

.context-menu-title span,
.context-menu-title small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.context-menu-title span {
  font-size: 12.5px;
  font-weight: 660;
  letter-spacing: -0.01em;
}

.context-menu-title small {
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 620;
  letter-spacing: 0.05em;
  text-transform: uppercase;
}

/* ── Items ────────────────────────────────────────────────── */
.context-menu-item {
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

.context-menu-item:hover:not(:disabled),
.context-menu-item:focus-visible {
  background: var(--btn-primary-bg);
  color: #fff;
  outline: 0;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.context-menu-item:disabled {
  cursor: default;
  color: var(--text-faint);
  opacity: 0.45;
}

.context-menu-item--danger {
  color: var(--danger);
}

.context-menu-item--danger:hover:not(:disabled),
.context-menu-item--danger:focus-visible {
  background: var(--btn-danger-bg);
  color: #fff;
}

/* ── Separator ────────────────────────────────────────────── */
.context-menu-separator {
  height: 1px;
  margin: 4px 2px;
  background: var(--hairline);
}
</style>
