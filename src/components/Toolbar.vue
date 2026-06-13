<script setup>
import { computed } from 'vue';
import AppIcon from './AppIcon.vue';
import SidebarSelector from './SidebarSelector.vue';
import WorkIndicator from './WorkIndicator.vue';
import WorkspaceSelector from './WorkspaceSelector.vue';
import { createFolder, deleteItems } from '../composables/useFileOperations';
import { useDialog } from '../composables/useDialog';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { archiveDisplayName, isArchivePath } from '../utils/archivePaths';
import {
  deleteConfirmationOptions,
  shouldConfirmDelete,
} from '../utils/deleteConfirmation';
import {
  closeTauriWindow,
  getTauriWindow,
  minimizeTauriWindow,
  toggleMaximizeTauriWindow,
} from '../composables/useTauriWindow';

const store = useFileManagerStore();
const dialog = useDialog();

const activeTitle = computed(() => {
  const path = store.activePane?.currentPath || '~';

  if (isArchivePath(path)) {
    return archiveDisplayName(path) || 'Archive';
  }

  const cleanPath = path.replace(/\/+$/, '');
  const name = cleanPath.split('/').filter(Boolean).at(-1);
  return name || cleanPath || 'Home';
});
const activeDirectoryIsArchive = computed(() =>
  isArchivePath(store.effectiveDirectoryFor(store.activePaneId) || ''),
);
const activeSelectionHasArchiveEntries = computed(() =>
  store.operationEntriesFor(store.activePaneId).some((entry) => isArchivePath(entry.path)),
);

function startDragging(event) {
  if (event.button !== 0 || event.detail > 1) return;
  event.preventDefault();
  getTauriWindow()?.startDragging().catch(() => {});
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
  closeTauriWindow({ force: true }).catch(() => {});
}

function refreshActivePane() {
  store.reloadDirectoryInPanes(store.effectiveDirectoryFor(store.activePaneId), [store.activePaneId]);
}

function setActivePaneView(viewMode) {
  store.setPaneView(store.activePaneId, viewMode);
}

function joinPath(directory, name) {
  if (!directory || directory === '/') {
    return `/${name}`;
  }

  return directory.endsWith('/') ? `${directory}${name}` : `${directory}/${name}`;
}

async function copySelectedPath() {
  const entry = store.operationEntriesFor(store.activePaneId)[0];
  const path = entry?.path || store.effectiveDirectoryFor(store.activePaneId) || '';

  if (path && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(path);
  }
}

async function createFolderInActivePane() {
  const targetDirectory = store.effectiveDirectoryFor(store.activePaneId) || '~';

  if (isArchivePath(targetDirectory)) {
    await dialog.alert({
      title: 'New Folder Not Available',
      message: 'Archive contents are read-only while browsing.',
      variant: 'warning',
    });
    return;
  }

  const name = (await dialog.prompt({
    title: 'New Folder',
    icon: 'folder',
    message: targetDirectory,
    inputLabel: 'Name',
    inputValue: '',
    inputPlaceholder: 'New Folder',
    confirmLabel: 'Create',
    inputRequired: true,
  }))?.trim();

  if (!name) {
    return;
  }

  await createFolder(joinPath(targetDirectory, name));
  await store.reloadDirectoryInPanes(targetDirectory, [store.activePaneId]);
}

async function deleteSelection() {
  const entries = store.operationEntriesFor(store.activePaneId);

  if (entries.length === 0) {
    return;
  }

  if (entries.some((entry) => isArchivePath(entry.path))) {
    await dialog.alert({
      title: 'Delete Not Available',
      message: 'Archive contents are read-only while browsing.',
      variant: 'warning',
    });
    return;
  }

  const label = entries.length === 1 ? `"${entries[0].name}"` : `${entries.length} selected items`;
  const confirmed = shouldConfirmDelete(
    store.appSettings.confirmDelete,
    store.appSettings.deleteMode,
    entries,
  )
    ? await dialog.confirm(deleteConfirmationOptions({
        entries,
        deleteMode: store.appSettings.deleteMode,
        label,
        singleTitle: 'Delete Item',
        pluralTitle: 'Delete Items',
      }))
    : true;

  if (!confirmed) {
    return;
  }

  const touchedDirectories = [...new Set(
    entries.map((entry) => store.parentDirectoryFor(entry.path)).filter(Boolean),
  )];
  await deleteItems(entries.map((entry) => entry.path), store.appSettings.deleteMode);
  await Promise.all(touchedDirectories.map((path) => store.reloadDirectoryInPanes(path)));
}
</script>

<template>
  <header
    class="toolbar"
    aria-label="Application toolbar"
    @mousedown="startDragging"
    @dblclick="toggleMaximizeWindow"
  >
    <div class="toolbar-left">
      <div
        v-if="!store.sidebarVisible"
        class="toolbar-window-controls"
        aria-label="Window actions"
        @mousedown.stop
        @dblclick.stop
      >
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

      <SidebarSelector v-if="!store.sidebarVisible" />

      <WorkspaceSelector v-if="!store.sidebarVisible" />

      <div class="nav-cluster" aria-label="Navigation" @mousedown.stop>
        <button
          v-tooltip="{ text: 'Go back', shortcut: 'Alt Left' }"
          type="button"
          class="nav-button"
          aria-label="Back"
          :disabled="!store.canGoBack"
          @click="store.goBack()"
        >
          <AppIcon name="chevron-left" :size="18" :stroke-width="2.2" />
        </button>
        <button
          v-tooltip="{ text: 'Go forward', shortcut: 'Alt Right' }"
          type="button"
          class="nav-button"
          aria-label="Forward"
          :disabled="!store.canGoForward"
          @click="store.goForward()"
        >
          <AppIcon name="chevron-right" :size="18" :stroke-width="2.2" />
        </button>
      </div>

      <h1>{{ activeTitle }}</h1>
      <WorkIndicator />
    </div>

    <div class="toolbar-right" @mousedown.stop>
      <div class="seg-control" role="group" aria-label="View mode">
        <button
          v-tooltip="{ text: 'List view', shortcut: 'Ctrl F2' }"
          type="button"
          class="seg-btn"
          :class="{ active: store.activePane?.viewMode === 'list' }"
          aria-label="List view"
          @click="setActivePaneView('list')"
        >
          <AppIcon name="list" :size="15" :stroke-width="1.9" />
        </button>
        <button
          v-tooltip="{ text: 'Grid view', shortcut: 'Ctrl F1' }"
          type="button"
          class="seg-btn"
          :class="{ active: store.activePane?.viewMode === 'grid' }"
          aria-label="Grid view"
          @click="setActivePaneView('grid')"
        >
          <AppIcon name="grid" :size="15" :stroke-width="1.9" />
        </button>
        <button
          v-tooltip="'Column view'"
          type="button"
          class="seg-btn"
          :class="{ active: store.activePane?.viewMode === 'columns' }"
          aria-label="Column view"
          @click="setActivePaneView('columns')"
        >
          <AppIcon name="columns" :size="15" :stroke-width="1.9" />
        </button>
      </div>

      <div class="toolbar-divider" aria-hidden="true"></div>

      <div class="icon-group status-action-group" role="toolbar" aria-label="Display actions">
        <button
          v-tooltip="{ text: store.showHiddenFiles ? 'Hide hidden files' : 'Show hidden files', shortcut: 'Ctrl .' }"
          type="button"
          class="icon-btn"
          :class="{ active: store.showHiddenFiles }"
          aria-label="Toggle hidden files"
          @click="store.toggleHiddenFiles"
        >
          <AppIcon :name="store.showHiddenFiles ? 'eye-off' : 'eye'" :size="19" :stroke-width="1.8" />
        </button>
      </div>

      <div class="toolbar-divider toolbar-divider--soft" aria-hidden="true"></div>

      <div class="icon-group pane-action-group" role="toolbar" aria-label="Pane actions">
        <button
          v-tooltip="{ text: 'Open in other pane', shortcut: 'Ctrl Right' }"
          type="button"
          class="icon-btn"
          aria-label="Open focused directory in other pane"
          @click="store.openFocusedDirectoryInOtherPane()"
        >
          <AppIcon name="open-other-pane" :size="19" :stroke-width="1.7" />
        </button>
        <button
          v-tooltip="{ text: 'Refresh', shortcut: 'Ctrl R' }"
          type="button"
          class="icon-btn"
          aria-label="Refresh"
          @click="refreshActivePane"
        >
          <AppIcon name="refresh" :size="19" :stroke-width="1.8" />
        </button>
      </div>

      <div class="toolbar-divider toolbar-divider--soft" aria-hidden="true"></div>

      <div class="icon-group file-action-group" role="toolbar" aria-label="File actions">
        <button
          v-tooltip="{ text: store.canUndo ? `Undo ${store.undoLabel}` : 'Nothing to undo', shortcut: 'Ctrl Z' }"
          type="button"
          class="icon-btn"
          aria-label="Undo last operation"
          :disabled="!store.canUndo"
          @click="store.undoLastOperation()"
        >
          <AppIcon name="undo" :size="19" :stroke-width="1.8" />
        </button>
        <button
          v-tooltip="{ text: store.canRedo ? `Redo ${store.redoLabel}` : 'Nothing to redo', shortcut: 'Ctrl Shift Z' }"
          type="button"
          class="icon-btn"
          aria-label="Redo last operation"
          :disabled="!store.canRedo"
          @click="store.redoLastOperation()"
        >
          <AppIcon name="redo" :size="19" :stroke-width="1.8" />
        </button>
        <button
          v-tooltip="{ text: 'Copy path', shortcut: 'Ctrl Shift Enter' }"
          type="button"
          class="icon-btn"
          aria-label="Copy selected path"
          @click="copySelectedPath"
        >
          <AppIcon name="copy" :size="19" :stroke-width="1.8" />
        </button>
        <button
          v-tooltip="{ text: 'New folder', shortcut: 'F7' }"
          type="button"
          class="icon-btn"
          aria-label="New folder"
          :disabled="activeDirectoryIsArchive"
          @click="createFolderInActivePane"
        >
          <AppIcon name="folder-plus" :size="19" :stroke-width="1.8" />
        </button>
        <button
          v-tooltip="{ text: 'Delete', shortcut: 'F8' }"
          type="button"
          class="icon-btn"
          aria-label="Delete selected items"
          :disabled="activeSelectionHasArchiveEntries"
          @click="deleteSelection"
        >
          <AppIcon name="trash" :size="19" :stroke-width="1.8" />
        </button>
      </div>

      <label class="search-field">
        <AppIcon name="search" :size="14" />
        <input v-model="store.searchQuery" data-search-field type="search" placeholder="Search" />
      </label>

      <div class="toolbar-divider toolbar-divider--soft" aria-hidden="true"></div>

      <div class="icon-group panel-toggle-group" role="toolbar" aria-label="Panel toggles">
        <button
          v-tooltip="{ text: 'Toggle terminal', shortcut: 'Ctrl `' }"
          type="button"
          class="icon-btn"
          :class="{ active: store.terminalPanelVisible }"
          aria-label="Toggle terminal panel"
          @click="store.toggleTerminalPanel()"
        >
          <AppIcon name="terminal" :size="19" :stroke-width="1.8" />
        </button>
        <button
          v-tooltip="{ text: store.sidebarVisible ? 'Hide sidebar' : 'Show sidebar', shortcut: 'Ctrl B' }"
          type="button"
          class="icon-btn"
          :class="{ active: store.sidebarVisible }"
          aria-label="Toggle left sidebar"
          @click="store.toggleSidebar"
        >
          <AppIcon name="sidebar" :size="19" :stroke-width="1.8" />
        </button>
        <button
          v-tooltip="{ text: store.previewPanelVisible ? 'Hide preview' : 'Show preview', shortcut: 'Ctrl I' }"
          type="button"
          class="icon-btn"
          :class="{ active: store.previewPanelVisible }"
          aria-label="Toggle preview sidebar"
          @click="store.togglePreviewPanel"
        >
          <AppIcon name="panel-right" :size="19" :stroke-width="1.8" />
        </button>
      </div>

      <div class="toolbar-divider toolbar-divider--soft" aria-hidden="true"></div>

      <div class="icon-group settings-action-group" role="toolbar" aria-label="Settings">
        <button
          v-tooltip="{ text: 'Settings', shortcut: 'Ctrl ,' }"
          type="button"
          class="icon-btn"
          :class="{ active: store.settingsVisible }"
          aria-label="Open settings"
          @click="store.openSettings"
        >
          <AppIcon name="sliders" :size="19" :stroke-width="1.8" />
        </button>
      </div>
    </div>
  </header>
</template>

<style scoped>
/* ── Toolbar shell ────────────────────────────────────────── */
.toolbar {
  display: grid;
  grid-template-columns: minmax(180px, 1fr) auto;
  align-items: center;
  min-width: 0;
  height: 56px;
  padding: 0 14px 0 16px;
  border-bottom: 1px solid var(--separator);
  border-radius: 0;
  background: var(--toolbar-bg);
  box-shadow: inset 0 1px 0 var(--hairline);
  user-select: none;
}

/* ── Left cluster ─────────────────────────────────────────── */
.toolbar-left {
  display: flex;
  align-items: center;
  gap: 14px;
  min-width: 0;
}

/* ── Traffic lights shown when the sidebar is hidden ─────── */
.toolbar-window-controls {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 4px 0 2px;
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

.toolbar-window-controls:hover .window-control span {
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

/* ── Nav cluster ──────────────────────────────────────────── */
.nav-cluster {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-shrink: 0;
}

.nav-button {
  display: inline-flex;
  width: 28px;
  height: 34px;
  flex: 0 0 28px;
  align-items: center;
  justify-content: center;
  border-radius: 6px;
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition: background 80ms ease, color 80ms ease;
}

.nav-button:hover:not(:disabled) {
  background: var(--btn-hover);
  color: var(--text);
}

.nav-button:disabled {
  cursor: default;
  opacity: 0.35;
}

/* ── Title ───────────────────────────────────────────────── */
h1 {
  min-width: 0;
  max-width: 280px;
  overflow: hidden;
  margin: 0;
  color: var(--text);
  font-size: 17px;
  font-weight: 700;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ── Right cluster ───────────────────────────────────────── */
.toolbar-right {
  display: flex;
  align-items: center;
  justify-self: end;
  gap: 8px;
  margin-left: auto;
  min-width: 0;
  flex-shrink: 0;
}

/* ── Segmented control ───────────────────────────────────── */
.seg-control {
  display: flex;
  height: 38px;
  overflow: hidden;
  border-radius: 8px;
  border: 0;
  background: transparent;
  box-shadow: none;
}

.seg-btn {
  display: inline-flex;
  width: 42px;
  height: 100%;
  align-items: center;
  justify-content: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition: background 80ms ease, color 80ms ease;
}

.seg-btn + .seg-btn {
  border-left: 0;
}

.seg-btn:hover {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.seg-btn.active {
  background: var(--btn-active-bg);
  color: var(--text);
  box-shadow: var(--btn-active-shadow);
}

/* ── Thin divider ────────────────────────────────────────── */
.toolbar-divider {
  width: 1px;
  height: 30px;
  flex-shrink: 0;
  margin: 0 2px;
  background: var(--separator);
  box-shadow: 1px 0 0 var(--hairline);
}

.toolbar-divider--soft {
  margin-left: 0;
}

/* ── Borderless icon groups ──────────────────────────────── */
.icon-group {
  display: flex;
  align-items: center;
  gap: 3px;
}

.panel-toggle-group {
  gap: 3px;
}

.icon-btn {
  display: inline-flex;
  width: 31px;
  height: 34px;
  align-items: center;
  justify-content: center;
  border-radius: 6px;
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition: background 80ms ease, color 80ms ease;
}

.icon-btn:hover:not(:disabled) {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.icon-btn:active:not(:disabled) {
  background: var(--btn-active-bg);
}

.icon-btn:disabled {
  cursor: default;
  opacity: 0.32;
}

.icon-btn.active {
  color: var(--text);
  background: rgb(255 255 255 / 0.11);
}

/* ── Search field ────────────────────────────────────────── */
.search-field {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 38px;
  width: clamp(160px, 14vw, 274px);
  padding: 0 10px;
  margin-left: 4px;
  border-radius: 8px;
  border: 1px solid var(--input-border);
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--icon);
  cursor: text;
  transition: border-color 140ms ease, box-shadow 140ms ease;
}

.search-field:focus-within {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.search-field input {
  min-width: 0;
  width: 100%;
  border: 0;
  background: transparent;
  color: var(--text);
  font-size: 15px;
  font-weight: 520;
  outline: 0;
}

.search-field input::placeholder {
  color: var(--text-muted);
}
</style>
