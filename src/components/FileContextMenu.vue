<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';

const props = defineProps({
  entry: {
    type: Object,
    default: null,
  },
  targetDirectory: {
    type: String,
    default: '',
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
  canReveal: {
    type: Boolean,
    default: true,
  },
  canEditFile: {
    type: Boolean,
    default: false,
  },
  canCustomTools: {
    type: Boolean,
    default: true,
  },
  customTools: {
    type: Array,
    default: () => [],
  },
  canModify: {
    type: Boolean,
    default: true,
  },
  canRename: {
    type: Boolean,
    default: true,
  },
  canMove: {
    type: Boolean,
    default: true,
  },
  canBatchRename: {
    type: Boolean,
    default: false,
  },
  canConvertImages: {
    type: Boolean,
    default: false,
  },
  canVerifyChecksum: {
    type: Boolean,
    default: false,
  },
  canTag: {
    type: Boolean,
    default: false,
  },
  activeTagColor: {
    type: String,
    default: '',
  },
  pdfToolActions: {
    type: Array,
    default: () => [],
  },
  operationCount: {
    type: Number,
    default: 1,
  },
});

const emit = defineEmits(['action', 'close']);

// Finder-style fixed tag palette. Stored value is the CSS color the row dot uses.
const TAG_COLORS = [
  { name: 'Red', value: '#ff453a' },
  { name: 'Orange', value: '#ff9f0a' },
  { name: 'Yellow', value: '#ffd60a' },
  { name: 'Green', value: '#32d74b' },
  { name: 'Blue', value: '#0a84ff' },
  { name: 'Purple', value: '#bf5af2' },
  { name: 'Gray', value: '#98989d' },
];

const menuRef = ref(null);
const filterInput = ref(null);
const toolsItemRef = ref(null);
const toolsMenuRef = ref(null);
const toolsSubmenuOpen = ref(false);
const actionFilter = ref('');
const menuStyle = ref({
  left: `${props.position.x}px`,
  top: `${props.position.y}px`,
  maxHeight: 'calc(100vh - 16px)',
});
const toolsMenuStyle = ref({
  left: '0px',
  top: '0px',
  maxHeight: 'calc(100vh - 16px)',
});

const canOpenInNewTab = computed(() => props.entry?.kind === 'directory');
const hasSingleOperation = computed(() => props.operationCount === 1);
const canRenameItem = computed(() => props.canModify && props.canRename && hasSingleOperation.value);
const itemType = computed(() => (props.entry?.kind === 'directory' ? 'Folder' : 'File'));
const isDirectoryContext = computed(() => !props.entry && Boolean(props.targetDirectory));
const hasMenuTarget = computed(() => Boolean(props.entry) || isDirectoryContext.value);
const visibleCustomTools = computed(() =>
  props.customTools.filter((tool) => tool?.enabled !== false && tool?.name && tool?.command),
);
const normalizedActionFilter = computed(() => normalizeMenuText(actionFilter.value));
const visibleSubmenuTools = computed(() => {
  const query = normalizedActionFilter.value;

  if (!query) {
    return visibleCustomTools.value;
  }

  const matchingTools = visibleCustomTools.value.filter((tool) => customToolMatchesFilter(tool, query));

  if (matchingTools.length > 0) {
    return matchingTools;
  }

  return matchesQuery('tools custom commands external tools', query)
    ? visibleCustomTools.value
    : [];
});
const actionGroups = computed(() => {
  const entryKind = props.entry?.kind;
  const groups = [];

  if (isDirectoryContext.value) {
    groups.push({
      id: 'directory',
      items: [
        {
          id: 'newFolder',
          action: 'newFolder',
          label: 'New Folder',
          icon: 'folder-plus',
          shortcut: props.canModify ? 'F7' : undefined,
          disabled: !props.canModify,
          keywords: ['create directory f7'],
        },
        {
          id: 'refreshDirectory',
          action: 'refreshDirectory',
          label: 'Refresh Folder',
          icon: 'refresh',
          shortcut: 'Ctrl R',
          keywords: ['reload update'],
        },
      ],
    });

    groups.push({
      id: 'path',
      items: [
        {
          id: 'openDirectoryInNewTab',
          action: 'openDirectoryInNewTab',
          label: 'Open in New Tab',
          icon: 'plus',
          keywords: ['tab duplicate'],
        },
        {
          id: 'copyDirectoryPath',
          action: 'copyDirectoryPath',
          label: 'Copy Folder Path',
          icon: 'copy',
          keywords: ['clipboard pathname location current directory'],
        },
      ],
    });

    return groups;
  }

  if (
    props.canVerifyChecksum ||
    props.pdfToolActions.length > 0 ||
    props.canConvertImages ||
    visibleCustomTools.value.length > 0
  ) {
    groups.push({
      id: 'tools',
      items: [
        ...(props.canVerifyChecksum
          ? [{
              id: 'verifyChecksum',
              action: 'verifyChecksum',
              label: props.operationCount === 1 ? 'Verify Checksum…' : 'Compare Checksums…',
              icon: 'shield',
              keywords: ['checksum hash sha256 sha-256 verify integrity digest fingerprint compare'],
            }]
          : []),
        ...props.pdfToolActions.map((action) => ({
          id: action.id,
          action: action.action,
          label: action.label,
          icon: action.icon || 'file-text',
          keywords: action.keywords || [],
        })),
        ...(props.canConvertImages
          ? [{
              id: 'convertImages',
              action: 'convertImages',
              label: props.operationCount === 1 ? 'Convert Image...' : 'Convert Images...',
              icon: 'image',
              keywords: ['image convert format avif png jpeg jpg webp tiff bmp ico'],
            }]
          : []),
        ...(visibleCustomTools.value.length > 0
          ? [{
              id: 'tools',
              type: 'submenu',
              label: 'Tools',
              icon: 'tool',
              disabled: !props.canCustomTools,
              keywords: [
                'custom commands external tools',
                ...visibleCustomTools.value.flatMap((tool) => [tool.name, tool.command]),
              ],
            }]
          : []),
      ],
    });
  }

  groups.push({
    id: 'open',
    items: [
      {
        id: 'open',
        action: 'open',
        label: 'Open',
        icon: entryKind === 'directory' ? 'folder' : 'file',
        shortcut: 'Enter',
        keywords: ['default'],
      },
      ...(props.canEditFile
        ? [{
            id: 'editFile',
            action: 'editFile',
            label: 'Edit File',
            icon: 'file-code',
            shortcut: 'F4',
            keywords: ['editor f4 modify'],
          }]
        : []),
      {
        id: 'openWith',
        action: 'openWith',
        label: 'Open With…',
        icon: 'app',
        disabled: !props.canOpenWith,
        keywords: ['application app'],
      },
      {
        id: 'openInNewTab',
        action: 'openInNewTab',
        label: 'Open in New Tab',
        icon: 'plus',
        shortcut: canOpenInNewTab.value ? 'Ctrl Up' : undefined,
        disabled: !canOpenInNewTab.value,
        keywords: ['tab'],
      },
      {
        id: 'reveal',
        action: 'reveal',
        label: 'Reveal in File Manager',
        icon: 'folder',
        disabled: !props.canReveal,
        keywords: ['show locate finder explorer nautilus'],
      },
    ],
  });

  groups.push({
    id: 'path',
    items: [
      {
        id: 'copyPath',
        action: 'copyPath',
        label: 'Copy Path',
        icon: 'copy',
        shortcut: hasSingleOperation.value ? 'Ctrl Shift Enter' : undefined,
        keywords: ['clipboard pathname location'],
      },
      {
        id: 'rename',
        action: 'rename',
        label: 'Rename',
        icon: 'file',
        shortcut: canRenameItem.value ? 'F2' : undefined,
        disabled: !canRenameItem.value,
        keywords: ['name move'],
      },
      {
        id: 'batchRename',
        action: 'batchRename',
        label: 'Batch Rename...',
        icon: 'file-text',
        disabled: !props.canBatchRename,
        keywords: ['bulk multiple rename pattern replace number'],
      },
    ],
  });

  groups.push({
    id: 'archive',
    items: [
      {
        id: 'archive',
        action: 'archive',
        label: 'Create Archive',
        icon: 'archive',
        disabled: !props.canArchive,
        keywords: ['zip compress'],
      },
      {
        id: 'unarchive',
        action: 'unarchive',
        label: 'Extract Archive',
        icon: 'extract',
        disabled: !props.canUnarchive,
        keywords: ['unzip 7z tar decompress extract'],
      },
    ],
  });

  groups.push({
    id: 'transfer',
    items: [
      {
        id: 'copyToOtherPane',
        action: 'copyToOtherPane',
        label: 'Copy to Other Pane',
        icon: 'copy',
        shortcut: props.canTransfer ? 'F5' : undefined,
        disabled: !props.canTransfer,
        keywords: ['duplicate transfer other panel'],
      },
      {
        id: 'moveToOtherPane',
        action: 'moveToOtherPane',
        label: 'Move to Other Pane',
        icon: 'chevron-right',
        shortcut: props.canMove ? 'F6' : undefined,
        disabled: !props.canMove,
        keywords: ['transfer other panel'],
      },
    ],
  });

  groups.push({
    id: 'danger',
    items: [
      {
        id: 'delete',
        action: 'delete',
        label: 'Delete',
        icon: 'trash',
        shortcut: props.canModify ? 'F8' : undefined,
        disabled: !props.canModify,
        danger: true,
        keywords: ['remove trash'],
      },
    ],
  });

  return groups;
});
const filteredActionGroups = computed(() => {
  const query = normalizedActionFilter.value;

  return actionGroups.value
    .map((group) => ({
      ...group,
      items: group.items.filter((item) => !item.disabled && itemMatchesFilter(item, query)),
    }))
    .filter((group) => group.items.length > 0);
});
const hasVisibleActions = computed(() => filteredActionGroups.value.length > 0);
const firstEnabledFilteredItem = computed(() => (
  filteredActionGroups.value
    .flatMap((group) => group.items)
    .find((item) => !item.disabled) || null
));
const titleLabel = computed(() => {
  if (props.entry?.name) {
    return props.entry.name;
  }

  return directoryName(props.targetDirectory) || 'Current Folder';
});
const titleDetail = computed(() => (isDirectoryContext.value ? 'Folder' : itemType.value));
const menuAriaLabel = computed(() =>
  isDirectoryContext.value
    ? `${titleLabel.value} folder context menu`
    : `${titleLabel.value} context menu`,
);

function directoryName(path) {
  const value = String(path || '').replace(/\/+$/, '');

  if (!value || value === '/' || value === '~') {
    return value || 'Current Folder';
  }

  if (value.startsWith('remote://')) {
    const parts = value.slice('remote://'.length).split('/').filter(Boolean);
    return parts.at(-1) || parts[0] || value;
  }

  return value.split('/').filter(Boolean).at(-1)?.replace(/!$/, '') || value;
}

function normalizeMenuText(value) {
  return String(value || '').toLowerCase().replace(/\s+/g, ' ').trim();
}

function matchesQuery(value, query) {
  if (!query) {
    return true;
  }

  const haystack = normalizeMenuText(value);

  return query.split(' ').every((term) => haystack.includes(term));
}

function itemMatchesFilter(item, query) {
  return matchesQuery([item.label, item.shortcut, ...(item.keywords || [])].join(' '), query);
}

function customToolMatchesFilter(tool, query) {
  return matchesQuery(`${tool?.name || ''} ${tool?.command || ''}`, query);
}

function assignToolsItemRef(element) {
  toolsItemRef.value = element || null;
}

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

    updateToolsMenuPosition();
  });
}

function emitAction(action) {
  toolsSubmenuOpen.value = false;
  emit('action', action);
}

function clearActionFilter() {
  actionFilter.value = '';
  focusFilter();
}

function focusFilter() {
  nextTick(() => {
    filterInput.value?.focus?.({ preventScroll: true });
  });
}

function focusFirstMenuItem() {
  nextTick(() => {
    menuRef.value?.querySelector('.context-menu-item:not(:disabled)')?.focus();
  });
}

function focusFirstToolsMenuItem() {
  nextTick(() => {
    toolsMenuRef.value?.querySelector('.context-menu-item:not(:disabled)')?.focus?.({ preventScroll: true });
  });
}

function activeMenuRoot() {
  if (toolsSubmenuOpen.value && toolsMenuRef.value?.contains(document.activeElement)) {
    return toolsMenuRef.value;
  }

  return menuRef.value;
}

function keyboardMenuItems(root = activeMenuRoot()) {
  return Array.from(root?.querySelectorAll('.context-menu-item:not(:disabled)') || []);
}

function focusMenuItemAt(index, root = activeMenuRoot()) {
  const items = keyboardMenuItems(root);

  if (items.length === 0) {
    return;
  }

  const nextIndex = (index + items.length) % items.length;
  items[nextIndex]?.focus?.({ preventScroll: true });
}

function focusRelativeMenuItem(delta) {
  const root = activeMenuRoot();
  const items = keyboardMenuItems(root);

  if (items.length === 0) {
    return;
  }

  const currentIndex = items.indexOf(document.activeElement);
  focusMenuItemAt(currentIndex < 0 ? (delta < 0 ? items.length - 1 : 0) : currentIndex + delta, root);
}

function activateFirstFilteredItem() {
  const item = firstEnabledFilteredItem.value;

  if (!item) {
    return;
  }

  handleMenuItemClick(item);
}

function handleFilterEscape() {
  if (actionFilter.value) {
    clearActionFilter();
    return;
  }

  if (toolsSubmenuOpen.value) {
    toolsSubmenuOpen.value = false;
    return;
  }

  emit('close');
}

function handleMenuItemPointerEnter(item) {
  if (item.type === 'submenu') {
    openToolsSubmenu();
    return;
  }

  toolsSubmenuOpen.value = false;
}

function handleMenuItemClick(item) {
  if (!item || item.disabled) {
    return;
  }

  if (item.type === 'submenu') {
    openToolsSubmenu();
    focusFirstToolsMenuItem();
    return;
  }

  emitAction(item.action);
}

function handleMenuItemRight(event, item) {
  if (item.type !== 'submenu') {
    return;
  }

  event.preventDefault();
  openToolsSubmenu();
  focusFirstToolsMenuItem();
}

function updateToolsMenuPosition() {
  nextTick(() => {
    const item = toolsItemRef.value;

    if (!toolsSubmenuOpen.value || !item) {
      return;
    }

    const margin = 8;
    const gap = 5;
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const itemRect = item.getBoundingClientRect();
    const submenu = toolsMenuRef.value;
    const submenuRect = submenu?.getBoundingClientRect();
    const submenuWidth = submenuRect?.width || 220;
    const submenuHeight = Math.min(submenu?.scrollHeight || 120, viewportHeight - margin * 2);
    const fitsRight = itemRect.right + gap + submenuWidth + margin <= viewportWidth;
    const left = fitsRight
      ? itemRect.right + gap
      : Math.max(margin, itemRect.left - submenuWidth - gap);
    const top = Math.min(
      Math.max(margin, itemRect.top - 5),
      Math.max(margin, viewportHeight - submenuHeight - margin),
    );

    toolsMenuStyle.value = {
      left: `${left}px`,
      top: `${top}px`,
      maxHeight: `${Math.max(1, viewportHeight - margin * 2)}px`,
    };
  });
}

function openToolsSubmenu() {
  if (visibleSubmenuTools.value.length === 0 || !props.canCustomTools) {
    toolsSubmenuOpen.value = false;
    return;
  }

  toolsSubmenuOpen.value = true;
  updateToolsMenuPosition();
}

function handleWindowPointerDown(event) {
  if (
    !menuRef.value?.contains(event.target) &&
    !toolsMenuRef.value?.contains(event.target)
  ) {
    toolsSubmenuOpen.value = false;
    emit('close');
  }
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault();

    if (toolsSubmenuOpen.value) {
      toolsSubmenuOpen.value = false;
      return;
    }

    emit('close');
    return;
  }

  if (event.key === 'ArrowDown') {
    event.preventDefault();
    focusRelativeMenuItem(1);
    return;
  }

  if (event.key === 'ArrowUp') {
    event.preventDefault();
    focusRelativeMenuItem(-1);
    return;
  }

  if (event.key === 'Home') {
    event.preventDefault();
    focusMenuItemAt(0);
    return;
  }

  if (event.key === 'End') {
    event.preventDefault();
    focusMenuItemAt(-1);
    return;
  }

  if (event.key === 'ArrowLeft' && toolsSubmenuOpen.value) {
    event.preventDefault();
    toolsSubmenuOpen.value = false;
    toolsItemRef.value?.focus?.({ preventScroll: true });
  }
}

watch(() => props.position, updatePosition, { deep: true });
watch(() => props.entry, () => {
  actionFilter.value = '';
  toolsSubmenuOpen.value = false;
  focusFilter();
});
watch(() => props.targetDirectory, () => {
  actionFilter.value = '';
  toolsSubmenuOpen.value = false;
  focusFilter();
});
watch(actionFilter, () => {
  toolsSubmenuOpen.value = false;
  updatePosition();
});
watch(visibleSubmenuTools, (tools) => {
  if (tools.length === 0) {
    toolsSubmenuOpen.value = false;
  } else if (toolsSubmenuOpen.value) {
    updateToolsMenuPosition();
  }
});

onMounted(() => {
  updatePosition();
  focusFilter();
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
      v-if="hasMenuTarget"
      ref="menuRef"
      class="context-menu"
      :style="menuStyle"
      role="menu"
      :aria-label="menuAriaLabel"
      @contextmenu.prevent
    >
      <label class="context-menu-filter">
        <AppIcon name="search" :size="14" :stroke-width="1.9" />
        <input
          ref="filterInput"
          v-model="actionFilter"
          type="search"
          autocomplete="off"
          spellcheck="false"
          placeholder="Filter actions..."
          aria-label="Filter context menu actions"
          @keydown.enter.prevent.stop="activateFirstFilteredItem"
          @keydown.down.prevent.stop="focusFirstMenuItem"
          @keydown.escape.prevent.stop="handleFilterEscape"
        />
        <button
          v-if="actionFilter"
          type="button"
          class="context-menu-filter-clear"
          aria-label="Clear context menu filter"
          @click="clearActionFilter"
        >
          <AppIcon name="x" :size="13" :stroke-width="2.2" />
        </button>
      </label>

      <div class="context-menu-title">
        <span>{{ titleLabel }}</span>
        <small>{{ titleDetail }}</small>
      </div>

      <template v-if="hasVisibleActions">
        <template
          v-for="(group, groupIndex) in filteredActionGroups"
          :key="group.id"
        >
          <div v-if="groupIndex > 0" class="context-menu-separator"></div>

          <button
            v-for="item in group.items"
            :key="item.id"
            :ref="item.type === 'submenu' ? assignToolsItemRef : undefined"
            type="button"
            role="menuitem"
            class="context-menu-item"
            :class="{
              'context-menu-item--submenu': item.type === 'submenu',
              'context-menu-item--shortcut': item.shortcut,
              'context-menu-item--open': item.type === 'submenu' && toolsSubmenuOpen,
              'context-menu-item--danger': item.danger,
            }"
            :disabled="item.disabled"
            :aria-haspopup="item.type === 'submenu' ? 'menu' : undefined"
            :aria-expanded="item.type === 'submenu' ? toolsSubmenuOpen : undefined"
            @pointerenter="handleMenuItemPointerEnter(item)"
            @focus="handleMenuItemPointerEnter(item)"
            @keydown.right="handleMenuItemRight($event, item)"
            @click="handleMenuItemClick(item)"
          >
            <AppIcon :name="item.icon" :size="16" />
            <span class="context-menu-item-label">{{ item.label }}</span>
            <kbd v-if="item.shortcut" class="kbd context-menu-shortcut">{{ item.shortcut }}</kbd>
            <AppIcon
              v-if="item.type === 'submenu'"
              class="context-submenu-chevron"
              name="chevron-right"
              :size="13"
              :stroke-width="2.1"
            />
          </button>
        </template>
      </template>

      <div v-else class="context-menu-empty">
        No matching actions
      </div>

      <template v-if="canTag && !normalizedActionFilter">
        <div class="context-menu-separator"></div>
        <div class="context-menu-tags" role="group" aria-label="Tag color">
          <button
            v-for="tag in TAG_COLORS"
            :key="tag.value"
            type="button"
            class="context-menu-swatch"
            :class="{ 'context-menu-swatch--active': activeTagColor === tag.value }"
            :style="{ '--swatch-color': tag.value }"
            :title="tag.name"
            :aria-label="`Tag ${tag.name}`"
            @click="emitAction(`tag:${tag.value}`)"
          ></button>
          <button
            type="button"
            class="context-menu-swatch context-menu-swatch--clear"
            :class="{ 'context-menu-swatch--active': !activeTagColor }"
            title="No tag"
            aria-label="Remove tag"
            @click="emitAction('tag:clear')"
          >
            <AppIcon name="x" :size="11" :stroke-width="2.4" />
          </button>
        </div>
      </template>
    </div>

    <div
      v-if="entry && toolsSubmenuOpen && visibleSubmenuTools.length > 0"
      ref="toolsMenuRef"
      class="context-menu context-submenu"
      :style="toolsMenuStyle"
      role="menu"
      aria-label="Tools"
      @contextmenu.prevent
    >
      <button
        v-for="tool in visibleSubmenuTools"
        :key="tool.id"
        type="button"
        role="menuitem"
        class="context-menu-item"
        @click="emitAction(`customTool:${tool.id}`)"
      >
        <AppIcon name="tool" :size="16" />
        <span class="context-menu-item-label">{{ tool.name }}</span>
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.context-menu {
  position: fixed;
  z-index: 2000;
  min-width: 224px;
  max-width: min(292px, calc(100vw - 16px));
  overflow-x: hidden;
  overflow-y: auto;
  overscroll-behavior: contain;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  padding: 5px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  animation: ctx-appear 130ms cubic-bezier(0.2, 0, 0, 1) both;
  transform-origin: top left;
  scrollbar-width: thin;
  scrollbar-color: var(--control-border) transparent;
}

.context-menu:not(.context-submenu) {
  width: min(292px, calc(100vw - 16px));
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

/* ── Filter ───────────────────────────────────────────────── */
.context-menu-filter {
  display: grid;
  grid-template-columns: 16px minmax(0, 1fr) auto;
  align-items: center;
  gap: 8px;
  min-height: 34px;
  margin: 1px 1px 5px;
  padding: 0 6px 0 10px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text-faint);
}

.context-menu-filter:focus-within {
  border-color: var(--accent-border);
  color: var(--text-muted);
  box-shadow:
    var(--input-shadow),
    var(--accent-focus-ring);
}

.context-menu-filter input {
  width: 100%;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--text);
  font: inherit;
  font-size: 12.5px;
  font-weight: 520;
  letter-spacing: 0;
}

.context-menu-filter input::placeholder {
  color: var(--text-faint);
}

.context-menu-filter input::-webkit-search-cancel-button {
  appearance: none;
}

.context-menu-filter-clear {
  display: grid;
  place-items: center;
  width: 22px;
  height: 22px;
  border-radius: 7px;
  background: transparent;
  color: var(--text-faint);
}

.context-menu-filter-clear:hover,
.context-menu-filter-clear:focus-visible {
  background: var(--btn-hover);
  color: var(--text);
  outline: 0;
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

.context-menu-item-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.context-menu-item--shortcut {
  grid-template-columns: 18px minmax(0, 1fr) auto;
}

.context-menu-item--submenu {
  grid-template-columns: 18px minmax(0, 1fr) 14px;
}

.context-menu-shortcut {
  justify-self: end;
  min-width: max-content;
  pointer-events: none;
}

.context-submenu-chevron {
  justify-self: end;
  opacity: 0.72;
}

.context-menu-item--submenu:hover:not(:disabled) .context-submenu-chevron,
.context-menu-item--submenu:focus-visible .context-submenu-chevron,
.context-menu-item--open .context-submenu-chevron {
  opacity: 1;
}

.context-menu-item:hover:not(:disabled),
.context-menu-item:focus-visible,
.context-menu-item--open:not(:disabled) {
  background: var(--btn-primary-bg);
  color: #fff;
  outline: 0;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.context-menu-item:hover:not(:disabled) .context-menu-shortcut,
.context-menu-item:focus-visible .context-menu-shortcut,
.context-menu-item--open:not(:disabled) .context-menu-shortcut {
  border-color: rgb(255 255 255 / 0.28);
  background: rgb(255 255 255 / 0.14);
  box-shadow: none;
  color: rgb(255 255 255 / 0.82);
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

.context-submenu {
  z-index: 2001;
  min-width: 190px;
  max-width: min(280px, calc(100vw - 16px));
}

.context-submenu .context-menu-item {
  min-height: 28px;
}

.context-menu-empty {
  padding: 12px 10px 11px;
  color: var(--text-faint);
  font-size: 12px;
  font-weight: 560;
  text-align: center;
}

/* ── Separator ────────────────────────────────────────────── */
.context-menu-separator {
  height: 1px;
  margin: 4px 2px;
  background: var(--hairline);
}

.context-menu-tags {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 5px 8px 6px;
}

.context-menu-swatch {
  display: grid;
  place-items: center;
  width: 19px;
  height: 19px;
  padding: 0;
  appearance: none;
  -webkit-appearance: none;
  border-radius: 50%;
  background: var(--swatch-color, transparent);
  box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.22);
  cursor: pointer;
  transition: transform 90ms ease, box-shadow 90ms ease;
}

.context-menu-swatch:hover {
  transform: scale(1.18);
}

.context-menu-swatch--active {
  box-shadow:
    0 0 0 2px var(--popover-bg),
    0 0 0 4px var(--accent);
}

.context-menu-swatch--clear {
  background: transparent;
  color: var(--text-muted);
  box-shadow: inset 0 0 0 1px var(--control-border);
}

.context-menu-swatch--clear:hover {
  color: var(--text);
}
</style>
