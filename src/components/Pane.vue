<script setup>
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref } from 'vue';
import AppIcon from './AppIcon.vue';
import FileContextMenu from './FileContextMenu.vue';
import FileList from './FileList.vue';
import TabContextMenu from './TabContextMenu.vue';
import {
  archiveItems,
  deleteItems,
  editFile,
  listOpenWithApps,
  listDirectory,
  openWithApp,
  openWithDefaultApp,
  revealInFileManager,
  runCustomTool,
  unarchiveItems,
} from '../composables/useFileOperations';
import { useDialog } from '../composables/useDialog';
import {
  cleanPath,
  dropEffectFromEvent,
  forcedTransferModeFromEvent,
  useFileTransferGuards,
} from '../composables/useFileTransferGuards';
import { useFileManagerStore } from '../stores/fileManagerStore';
import {
  archiveBreadcrumbs,
  archiveRootPath,
  isArchiveEntry,
  isArchivePath,
} from '../utils/archivePaths';
import { extensionForName } from '../utils/fileTypes';

const CreateArchiveDialog = defineAsyncComponent(() => import('./CreateArchiveDialog.vue'));
const OpenWithDialog = defineAsyncComponent(() => import('./OpenWithDialog.vue'));

const FILE_DRAG_MIME = 'application/x-carelo-files';
const TAB_DRAG_MIME = 'application/x-carelo-tab';
const DEFAULT_FAVORITE_GROUP_ID = 'favorites';
const POINTER_DRAG_THRESHOLD = 6;

const props = defineProps({
  paneId: {
    type: String,
    required: true,
  },
  title: {
    type: String,
    required: true,
  },
});

const store = useFileManagerStore();
const dialog = useDialog();
const transfers = useFileTransferGuards();
const pane = computed(() => store.panes[props.paneId]);
const activeTab = computed(() => store.activeTabFor(props.paneId));
const isActive = computed(() => store.activePaneId === props.paneId);
const entries = computed(() => store.visibleEntriesFor(props.paneId));
const rawEntryCount = computed(() => activeTab.value?.entries?.length || 0);
const activeSearchQuery = computed(() => store.searchQuery.trim());
const SIZE_UNITS = [
  { unit: 'TB', value: 1024 ** 4 },
  { unit: 'GB', value: 1024 ** 3 },
  { unit: 'MB', value: 1024 ** 2 },
  { unit: 'KB', value: 1024 },
];

function itemCountLabel(count) {
  return `${count} ${count === 1 ? 'item' : 'items'}`;
}

function compactSize(bytes) {
  const size = Number(bytes);

  if (!Number.isFinite(size)) {
    return '';
  }

  if (size < 1024) {
    return `${size} B`;
  }

  const unit = SIZE_UNITS.find((candidate) => size >= candidate.value) || SIZE_UNITS.at(-1);
  const value = size / unit.value;
  const maximumFractionDigits = value >= 10 ? 0 : 1;

  return `${new Intl.NumberFormat(undefined, { maximumFractionDigits }).format(value)} ${unit.unit}`;
}

function totalKnownSize(targetEntries) {
  if (targetEntries.length === 0) {
    return null;
  }

  const sizes = targetEntries.map((entry) => Number(entry.size));

  if (sizes.some((size) => !Number.isFinite(size))) {
    return null;
  }

  return sizes.reduce((total, size) => total + size, 0);
}

function remoteBreadcrumbs(path) {
  const rest = String(path || '').replace(/\/+$/, '').slice('remote://'.length);
  const parts = rest.split('/').filter(Boolean);
  const volumeId = parts[0] || '';

  if (!volumeId) {
    return [{ label: 'Remote', path: 'remote://' }];
  }

  const rootPath = `remote://${volumeId}/`;
  const volumes = Array.isArray(store.volumes) ? store.volumes : [];
  const volume = volumes.find((candidate) => candidate.path === rootPath);
  const crumbs = [{
    label: volume?.name || volumeId,
    path: rootPath,
  }];

  let objectPath = '';

  for (const part of parts.slice(1)) {
    objectPath = objectPath ? `${objectPath}/${part}` : part;
    crumbs.push({
      label: part,
      path: `remote://${volumeId}/${objectPath}`,
    });
  }

  return crumbs;
}

const breadcrumbs = computed(() => {
  const rawPath = String(activeTab.value?.currentPath || '~');

  if (isArchivePath(rawPath)) {
    return archiveBreadcrumbs(rawPath);
  }

  if (rawPath.startsWith('remote://')) {
    return remoteBreadcrumbs(rawPath);
  }

  const currentPath = rawPath.replace(/\/+$/, '') || '/';

  if (currentPath === '/') {
    return [{ label: 'Root', path: '/' }];
  }

  if (currentPath === '~' || currentPath.startsWith('~/')) {
    const parts = currentPath.split('/').filter(Boolean);

    return parts.map((part, index) => ({
      label: index === 0 ? 'Home' : part,
      path: index === 0 ? '~' : parts.slice(0, index + 1).join('/'),
    }));
  }

  const parts = currentPath.split('/').filter(Boolean);
  const displayOffset = parts[0] === 'Users' ? 1 : 0;

  return parts.slice(displayOffset).map((part, index) => {
    const originalIndex = index + displayOffset;

    return {
      label: part,
      path: `/${parts.slice(0, originalIndex + 1).join('/')}`,
    };
  });
});
const summaryParts = computed(() => {
  const source = activeSummarySource();
  const totalCount = source.entries?.length || 0;
  const rawCount = source.rawEntryCount ?? totalCount;
  const hasSearchFilter = Boolean(source.searchQuery || activeSearchQuery.value);
  const hiddenItemsFiltered = source.showHiddenFiles === false && rawCount > 0 && totalCount === 0;
  const selectedEntries = source.selectedEntries || [];
  const selectedCount = selectedEntries.length;

  if (source.loading) {
    return ['Loading folder'];
  }

  if (source.error) {
    return ['Unable to load folder'];
  }

  if (totalCount === 0 && hasSearchFilter && rawCount > 0) {
    return ['No matching items'];
  }

  if (hiddenItemsFiltered) {
    return ['Only hidden items'];
  }

  if (totalCount === 0) {
    return ['Empty folder'];
  }

  if (selectedCount === 0) {
    return [itemCountLabel(totalCount)];
  }

  const parts = [
    selectedCount === totalCount
      ? `${itemCountLabel(totalCount)} selected`
      : `${selectedCount} selected`,
  ];

  if (selectedCount !== totalCount) {
    parts.push(`${itemCountLabel(totalCount)} total`);
  }

  const selectedSize = totalKnownSize(selectedEntries);

  if (selectedSize !== null) {
    parts.push(compactSize(selectedSize));
  }

  return parts;
});
const sortDirectionLabel = computed(() =>
  activeTab.value?.sortDirection === 'asc' ? 'Ascending' : 'Descending',
);
const contextMenu = ref(null);
const tabContextMenu = ref(null);
const archiveDialog = ref({
  visible: false,
  entries: [],
  directory: '',
  existingNames: [],
});
const openWithDialog = ref({
  visible: false,
  entry: null,
  context: null,
  loading: false,
  error: '',
});
let openWithRequestId = 0;
const columnSummary = ref(null);
const draggedTabId = ref(null);
const tabDropIndex = ref(null);
const dragGhost = ref({
  visible: false,
  x: 0,
  y: 0,
  count: 0,
  label: '',
  kind: 'file',
  operation: 'auto',
});
let pointerDrag = null;
let pointerDragCleanup = null;
let fileDragClearTimer = null;
const otherPaneId = computed(() => (props.paneId === 'left' ? 'right' : 'left'));
const canTransferToOtherPane = computed(() => {
  const targetDirectory = store.effectiveDirectoryFor(otherPaneId.value);

  return Boolean(targetDirectory && !isArchivePath(targetDirectory));
});
const isFileDragActive = computed(() => Boolean(store.dragOperation?.entries?.length));
const draggedPaths = computed(() => store.dragOperation?.entries?.map((entry) => entry.path) || []);
const canModifyContext = computed(() =>
  contextOperationEntries(contextMenu.value).every((item) => !isArchivePath(item.path)),
);
const canMoveContext = computed(() =>
  canTransferToOtherPane.value
    && contextOperationEntries(contextMenu.value).every((item) => !isArchivePath(item.path)),
);
const canArchiveContext = computed(() => {
  const operationEntries = contextOperationEntries(contextMenu.value);

  return operationEntries.length > 0 && operationEntries.every(isLocalEntry);
});
const canUnarchiveContext = computed(() => {
  const operationEntries = contextOperationEntries(contextMenu.value);

  return operationEntries.length > 0 && operationEntries.every((item) => isLocalEntry(item) && isExtractableArchiveEntry(item));
});
const canOpenWithContext = computed(() => {
  const operationEntries = contextOperationEntries(contextMenu.value);

  return operationEntries.length === 1 && operationEntries[0]?.kind === 'file' && isLocalEntry(operationEntries[0]);
});
const canEditFileContext = computed(() => {
  const operationEntries = contextOperationEntries(contextMenu.value);

  return operationEntries.length === 1 && operationEntries[0]?.kind === 'file' && !isArchivePath(operationEntries[0].path);
});
const configuredCustomTools = computed(() =>
  (store.appSettings.customTools || []).filter((tool) => tool?.enabled !== false && tool?.name && tool?.command),
);
const availableCustomTools = computed(() => {
  const operationEntries = contextOperationEntries(contextMenu.value);

  if (operationEntries.length === 0 || operationEntries.some((item) => !isLocalEntry(item))) {
    return [];
  }

  return configuredCustomTools.value.filter((tool) =>
    operationEntries.every((entry) => customToolAppliesToEntry(tool, entry)),
  );
});
const canRunCustomToolContext = computed(() => {
  return availableCustomTools.value.length > 0;
});

function rootSummarySource() {
  const tab = activeTab.value;
  const visibleEntries = entries.value;
  const selectedPaths = Array.isArray(tab?.selectedPaths) ? tab.selectedPaths : [];
  const selectedPathSet = selectedPaths.length > 0 ? new Set(selectedPaths) : null;
  const selectedEntries = selectedPaths.length > 0
    ? visibleEntries.filter((entry) => selectedPathSet.has(entry.path))
    : Number.isInteger(tab?.selectedIndex) && tab.selectedIndex >= 0
      ? [visibleEntries[tab.selectedIndex]].filter(Boolean)
      : [];

  return {
    loading: tab?.loading,
    error: tab?.error || '',
    entries: visibleEntries,
    rawEntryCount: rawEntryCount.value,
    searchQuery: activeSearchQuery.value,
    showHiddenFiles: store.showHiddenFiles,
    selectedEntries,
  };
}

function activeSummarySource() {
  if (activeTab.value?.viewMode === 'columns' && columnSummary.value) {
    return columnSummary.value;
  }

  return rootSummarySource();
}

function updateColumnSummary(summary) {
  columnSummary.value = summary;
  store.setColumnSelectionState(props.paneId, summary
    ? {
        path: summary.path,
        entries: summary.selectedEntries || [],
        focusedEntry: summary.focusedEntry || summary.selectedEntries?.[0] || null,
      }
    : null);
}

onMounted(async () => {
  await store.initialize();
  store.loadPane(props.paneId);
});

onUnmounted(() => {
  cleanupPointerDrag();
  clearFileDragNow();
});

function activateTab(tabId) {
  store.setActiveTab(props.paneId, tabId);
}

function addTab() {
  store.addPaneTab(props.paneId);
}

function closeTab(tabId) {
  store.closePaneTab(props.paneId, tabId);
}

function tabForId(tabId) {
  return pane.value?.tabs.find((tab) => tab.id === tabId) || null;
}

function showTabContextMenu(tab, event) {
  event.preventDefault();
  event.stopPropagation();
  store.setActivePane(props.paneId);
  closeContextMenu();
  tabContextMenu.value = {
    tabId: tab.id,
    position: {
      x: event.clientX,
      y: event.clientY,
    },
  };
}

function closeTabContextMenu() {
  tabContextMenu.value = null;
}

async function handleTabContextAction(action) {
  const menu = tabContextMenu.value;
  const tab = tabForId(menu?.tabId);

  closeTabContextMenu();

  if (!tab) {
    return;
  }

  try {
    if (action === 'copyPath') {
      await copyPathToClipboard(tab.currentPath);
      return;
    }

    if (action === 'duplicate') {
      store.duplicatePaneTab(props.paneId, tab.id);
      return;
    }

    if (action === 'openInOtherPane') {
      store.addPaneTab(otherPaneId.value, tab.currentPath);
      return;
    }

    if (action === 'moveToOtherPane') {
      store.movePaneTab(props.paneId, tab.id, otherPaneId.value);
      return;
    }

    if (action === 'close') {
      store.closePaneTab(props.paneId, tab.id);
      return;
    }

    if (action === 'closeOthers') {
      store.closeOtherPaneTabs(props.paneId, tab.id);
    }
  } catch (error) {
    console.error(error);
    await dialog.alert({
      title: 'Tab Action Failed',
      message: error?.message || 'The tab action could not be completed.',
      variant: 'warning',
    });
  }
}

function dataTransferTypes(event) {
  return Array.from(event?.dataTransfer?.types || []);
}

function isTabDragEvent(event) {
  return dataTransferTypes(event).includes(TAB_DRAG_MIME);
}

function tabDragPayload(tab) {
  return {
    sourcePaneId: props.paneId,
    tabId: tab.id,
  };
}

function readTabDragPayload(event) {
  const rawPayload = event?.dataTransfer?.getData(TAB_DRAG_MIME);

  if (!rawPayload) {
    return null;
  }

  try {
    const payload = JSON.parse(rawPayload);

    if (!payload.sourcePaneId || !payload.tabId) {
      return null;
    }

    return payload;
  } catch {
    return null;
  }
}

function tabIndexFromStripEvent(event) {
  const tabStrip = event.currentTarget?.closest?.('.pane-tabs') || event.currentTarget;
  const tabElements = Array.from(tabStrip?.querySelectorAll?.('.pane-tab') || []);
  const targetIndex = tabElements.findIndex((element) => {
    const rect = element.getBoundingClientRect();

    return event.clientX < rect.left + rect.width / 2;
  });

  return targetIndex >= 0 ? targetIndex : pane.value.tabs.length;
}

function tabIndexFromTabEvent(tab, event) {
  const index = pane.value.tabs.findIndex((candidate) => candidate.id === tab.id);

  if (index < 0) {
    return pane.value.tabs.length;
  }

  const rect = event.currentTarget.getBoundingClientRect();
  return event.clientX < rect.left + rect.width / 2 ? index : index + 1;
}

function setTabDropIndex(index) {
  if (tabDropIndex.value !== index) {
    tabDropIndex.value = index;
  }
}

function isDragPointInsideElement(event, element) {
  if (!element || typeof event.clientX !== 'number' || typeof event.clientY !== 'number') {
    return false;
  }

  const rect = element.getBoundingClientRect();

  return (
    event.clientX >= rect.left &&
    event.clientX <= rect.right &&
    event.clientY >= rect.top &&
    event.clientY <= rect.bottom
  );
}

function handleTabDragStart(tab, event) {
  const payload = tabDragPayload(tab);

  closeTabContextMenu();
  clearFileDragNow();
  draggedTabId.value = tab.id;

  if (!event.dataTransfer) {
    return;
  }

  event.dataTransfer.effectAllowed = 'move';
  event.dataTransfer.dropEffect = 'move';
  event.dataTransfer.setData(TAB_DRAG_MIME, JSON.stringify(payload));
  event.dataTransfer.setData('text/plain', tab.currentPath);
}

function handleTabDragEnd() {
  draggedTabId.value = null;
  setTabDropIndex(null);
}

function handleTabDragOver(tab, event) {
  if (!isTabDragEvent(event)) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  event.dataTransfer.dropEffect = 'move';
  setTabDropIndex(tabIndexFromTabEvent(tab, event));
}

function handleTabStripDragOver(event) {
  if (!isTabDragEvent(event)) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  event.dataTransfer.dropEffect = 'move';
  setTabDropIndex(tabIndexFromStripEvent(event));
}

function handleTabStripDragLeave(event) {
  if (!isTabDragEvent(event)) {
    return;
  }

  if (
    event.relatedTarget instanceof Node &&
    event.currentTarget.contains(event.relatedTarget)
  ) {
    return;
  }

  if (isDragPointInsideElement(event, event.currentTarget)) {
    return;
  }

  setTabDropIndex(null);
}

function dropTabAt(index, event) {
  const payload = readTabDragPayload(event);

  if (!payload) {
    setTabDropIndex(null);
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  store.movePaneTab(payload.sourcePaneId, payload.tabId, props.paneId, index);
  draggedTabId.value = null;
  setTabDropIndex(null);
}

function handleTabDrop(tab, event) {
  if (!isTabDragEvent(event)) {
    return;
  }

  dropTabAt(tabIndexFromTabEvent(tab, event), event);
}

function handleTabStripDrop(event) {
  if (!isTabDragEvent(event)) {
    return;
  }

  dropTabAt(tabIndexFromStripEvent(event), event);
}

function setSortKey(event) {
  store.setPaneSortKey(props.paneId, event.target.value);
}

function toggleSortDirection() {
  store.togglePaneSortDirection(props.paneId);
}

function navigateToBreadcrumb(path) {
  store.setPanePath(props.paneId, path);
}

function handleFileSelect(payload) {
  const index = typeof payload === 'number' ? payload : payload?.index;
  const event = typeof payload === 'object' ? payload.event : null;

  if (!Number.isInteger(index)) {
    return;
  }

  store.setActivePane(props.paneId);

  if (event?.shiftKey) {
    store.selectEntryRange(props.paneId, index);
    return;
  }

  if (event?.metaKey || event?.ctrlKey) {
    store.toggleEntrySelection(props.paneId, index);
    return;
  }

  store.selectEntry(props.paneId, index);
}

function handleBackgroundClick(payload) {
  const event = payload?.event;

  store.setActivePane(props.paneId);

  if (event?.shiftKey || event?.metaKey || event?.ctrlKey) {
    return;
  }

  store.selectEntry(props.paneId, -1);
}

function showContextMenu(payload) {
  store.setActivePane(props.paneId);
  closeTabContextMenu();

  if (Number.isInteger(payload.index)) {
    if (!store.isEntrySelected(props.paneId, payload.index)) {
      store.selectEntry(props.paneId, payload.index);
    }
  } else {
    store.setColumnPreviewEntry(props.paneId, payload.entry);
  }

  contextMenu.value = {
    entry: payload.entry,
    index: payload.index,
    operationEntries: payload.operationEntries,
    position: {
      x: payload.x,
      y: payload.y,
    },
  };
}

function closeContextMenu() {
  contextMenu.value = null;
}

function readDragPayload(event) {
  if (store.dragOperation?.entries?.length) {
    return store.dragOperation;
  }

  const rawPayload = event?.dataTransfer?.getData(FILE_DRAG_MIME);

  if (!rawPayload) {
    return null;
  }

  try {
    const payload = JSON.parse(rawPayload);

    if (!Array.isArray(payload.entries) || !payload.sourcePaneId) {
      return null;
    }

    return payload;
  } catch {
    return null;
  }
}

function isFileTransferDragEvent(event) {
  return Boolean(store.dragOperation?.entries?.length)
    || dataTransferTypes(event).includes(FILE_DRAG_MIME);
}

function draggableEntriesFor(payload) {
  if (Array.isArray(payload.operationEntries) && payload.operationEntries.length > 0) {
    return payload.operationEntries;
  }

  const selectedEntries = Number.isInteger(payload.index)
    ? store.selectedEntriesFor(props.paneId)
    : [];
  const isSelectedDrag = selectedEntries.some((entry) => entry.path === payload.entry.path);

  if (isSelectedDrag) {
    return selectedEntries;
  }

  if (Number.isInteger(payload.index)) {
    store.selectEntry(props.paneId, payload.index);
  } else {
    store.setColumnPreviewEntry(props.paneId, payload.entry);
  }

  return [payload.entry];
}

function fileUriForLocalPath(path) {
  const value = String(path || '');

  if (!value || value.startsWith('remote://') || isArchivePath(value)) {
    return '';
  }

  return `file://${value.split('/').map((part) => encodeURIComponent(part)).join('/')}`;
}

function nativeFileUriList(entries) {
  return entries
    .map((entry) => fileUriForLocalPath(entry.path))
    .filter(Boolean)
    .join('\n');
}

function cancelFileDragClear() {
  if (!fileDragClearTimer) {
    return;
  }

  clearTimeout(fileDragClearTimer);
  fileDragClearTimer = null;
}

function clearFileDragNow() {
  cancelFileDragClear();
  store.clearFileDrag();
}

function scheduleFileDragClear() {
  cancelFileDragClear();
  fileDragClearTimer = window.setTimeout(() => {
    fileDragClearTimer = null;
    store.clearFileDrag();
  }, 120);
}

function writeDragPayload(event, entries) {
  const requestedMode = forcedTransferModeFromEvent(event);
  cancelFileDragClear();
  store.startFileDrag(props.paneId, entries, requestedMode);

  const payload = {
    id: store.dragOperation?.id || null,
    sourcePaneId: props.paneId,
    requestedMode,
    entries: entries.map((entry) => ({
      name: entry.name,
      path: entry.path,
      kind: entry.kind,
      isSymlink: entry.isSymlink,
    })),
  };

  if (!event.dataTransfer) {
    return;
  }

  event.dataTransfer.effectAllowed = 'copyMove';
  event.dataTransfer.dropEffect = dropEffectFromEvent(event, requestedMode || 'move');
  event.dataTransfer.setData(FILE_DRAG_MIME, JSON.stringify(payload));
  event.dataTransfer.setData('text/plain', entries.map((entry) => entry.path).join('\n'));

  const uriList = nativeFileUriList(entries);

  if (uriList) {
    event.dataTransfer.setData('text/uri-list', uriList);
    event.dataTransfer.setData('x-special/gnome-copied-files', `copy\n${uriList}`);
  }
}

function closestFromElements(elements, selector) {
  for (const element of elements) {
    const match = element?.closest?.(selector);

    if (match) {
      return match;
    }
  }

  return null;
}

function pointerElements(event) {
  if (!isPointerInsideViewport(event)) {
    return [];
  }

  if (typeof document.elementsFromPoint === 'function') {
    return document.elementsFromPoint(event.clientX, event.clientY);
  }

  return [document.elementFromPoint(event.clientX, event.clientY)].filter(Boolean);
}

function hasPointerCoordinates(event) {
  return typeof event?.clientX === 'number' && typeof event?.clientY === 'number';
}

function isPointerInsideViewport(event) {
  if (!hasPointerCoordinates(event) || typeof window === 'undefined') {
    return false;
  }

  return event.clientX >= 0 &&
    event.clientY >= 0 &&
    event.clientX <= window.innerWidth &&
    event.clientY <= window.innerHeight;
}

function pointerFavoriteDrop(elements, event) {
  const favoriteZone = closestFromElements(elements, '[data-favorite-drop-zone]');

  if (!favoriteZone) {
    return null;
  }

  const groupId = favoriteZone.dataset.favoriteGroupId || DEFAULT_FAVORITE_GROUP_ID;
  const favoriteItem = closestFromElements(elements, '[data-favorite-index]');
  const groupFavoriteCount = store.favorites.filter((favorite) =>
    (favorite.groupId || DEFAULT_FAVORITE_GROUP_ID) === groupId,
  ).length;
  let index = groupFavoriteCount;

  if (favoriteItem && favoriteZone.contains(favoriteItem)) {
    const candidateIndex = Number(favoriteItem.dataset.favoriteIndex);

    if (Number.isInteger(candidateIndex)) {
      const rect = favoriteItem.getBoundingClientRect();
      index = event.clientY < rect.top + rect.height / 2
        ? candidateIndex
        : candidateIndex + 1;
    }
  }

  return {
    type: 'favorite',
    groupId,
    index: Number.isInteger(index) ? index : groupFavoriteCount,
  };
}

function pointerPaneDrop(elements) {
  const paneElement = closestFromElements(elements, '[data-pane-id]');

  if (!paneElement) {
    return null;
  }

  const targetPaneId = paneElement.dataset.paneId;
  const entryElement = closestFromElements(elements, '[data-drop-entry-path]');
  const entryBelongsToPane = entryElement && paneElement.contains(entryElement);
  const directoryElement = closestFromElements(elements, '[data-drop-directory-path]');
  const directoryBelongsToPane = directoryElement && paneElement.contains(directoryElement);
  const targetDirectory = entryBelongsToPane && entryElement.dataset.dropEntryKind === 'directory'
    ? entryElement.dataset.dropEntryPath
    : directoryBelongsToPane
      ? directoryElement.dataset.dropDirectoryPath
      : effectiveDirectory(targetPaneId);

  if (!targetPaneId || !targetDirectory) {
    return null;
  }

  return {
    type: 'pane',
    targetPaneId,
    targetDirectory,
  };
}

function isSameDirectoryTransfer(dragOperation, targetDirectory) {
  if (!dragOperation?.entries?.length || !targetDirectory) {
    return false;
  }

  const targetPath = cleanPath(targetDirectory);

  return dragOperation.entries.every((entry) =>
    cleanPath(store.parentDirectoryFor(entry.path)) === targetPath,
  );
}

function shouldTransferToPaneDrop(dragOperation, paneDrop, event = null) {
  if (!dragOperation?.entries?.length || !paneDrop?.targetPaneId || !paneDrop?.targetDirectory) {
    return false;
  }

  const requestedMode = forcedTransferModeFromEvent(event) || dragOperation.requestedMode;

  return requestedMode === 'copy' || !isSameDirectoryTransfer(dragOperation, paneDrop.targetDirectory);
}

function paneDropFromEvent(event) {
  if (hasPointerCoordinates(event)) {
    const paneDrop = pointerPaneDrop(pointerElements(event));

    if (paneDrop) {
      return paneDrop;
    }
  }

  const targetDirectory = effectiveDirectory();

  return targetDirectory
    ? {
        type: 'pane',
        targetPaneId: props.paneId,
        targetDirectory,
      }
    : null;
}

async function reloadPanesAfterPointerMove(sourcePaneId, targetPaneId, targetDirectory, operationEntries = []) {
  const paneIds = [...new Set([sourcePaneId, targetPaneId].filter(Boolean))];
  const touchedDirectories = [
    targetDirectory,
    ...parentDirectoriesForEntries(operationEntries),
  ];

  await refreshDirectories(touchedDirectories, paneIds);
  requestColumnRefreshes(touchedDirectories, paneIds);
}

async function transferEntriesToPointerDrop(event, dragOperation, paneDrop) {
  if (!dragOperation?.entries?.length || !paneDrop?.targetDirectory) {
    return false;
  }

  if (!shouldTransferToPaneDrop(dragOperation, paneDrop, event)) {
    return false;
  }

  if (!store.claimFileDrop(dragOperation.id)) {
    return false;
  }

  const payload = {
    id: dragOperation.id,
    sourcePaneId: dragOperation.sourcePaneId,
    requestedMode: dragOperation.requestedMode,
    entries: dragOperation.entries,
  };

  clearFileDragNow();

  try {
    const mode = await transfers.transferModeForEvent(
      event,
      payload.entries,
      paneDrop.targetDirectory,
    );
    const transferred = await transfers.transferEntries({
      mode,
      entries: payload.entries,
      targetDirectory: paneDrop.targetDirectory,
    });

    if (transferred) {
      await reloadPanesAfterPointerMove(
        payload.sourcePaneId,
        paneDrop.targetPaneId,
        paneDrop.targetDirectory,
        payload.entries,
      );
    }
  } catch (error) {
    console.error(error);
    await dialog.alert({
      title: 'Drag and Drop Failed',
      message: error?.message || 'The dragged items could not be dropped there.',
      variant: 'warning',
    });
  } finally {
    clearFileDragNow();
  }

  return true;
}

async function finishPointerFileDrop(event, state) {
  const elements = pointerElements(event);
  const favoriteDrop = pointerFavoriteDrop(elements, event);

  if (favoriteDrop) {
    if (!store.claimFileDrop(state.id)) {
      return;
    }

    const directories = state.entries.filter((entry) =>
      entry.kind === 'directory' && !isArchivePath(entry.path),
    );

    if (directories.length > 0) {
      await store.addFavoritesFromEntries(directories, favoriteDrop.index, favoriteDrop.groupId);
    }

    return;
  }

  const paneDrop = pointerPaneDrop(elements);

  if (!paneDrop) {
    return;
  }

  await transferEntriesToPointerDrop(event, state, paneDrop);
}

function ghostKindForEntries(operationEntries) {
  if (operationEntries.length !== 1) {
    return 'multiple';
  }

  return operationEntries[0]?.kind === 'directory' ? 'directory' : 'file';
}

function ghostIconName(kind) {
  if (kind === 'directory') {
    return 'folder';
  }

  if (kind === 'multiple') {
    return 'copy';
  }

  return 'file';
}

function ghostOperationLabel(operation) {
  if (operation === 'auto') {
    return 'Auto';
  }

  return operation === 'copy' ? 'Copy' : 'Move';
}

function updateDragGhost(event, operationEntries = []) {
  if (operationEntries.length === 0) {
    dragGhost.value = { ...dragGhost.value, visible: false };
    return;
  }

  const operation = forcedTransferModeFromEvent(event) || 'auto';
  dragGhost.value = {
    visible: true,
    x: event.clientX + 14,
    y: event.clientY + 14,
    count: operationEntries.length,
    label: operationEntries.length === 1
      ? operationEntries[0].name
      : `${operationEntries.length} items`,
    kind: ghostKindForEntries(operationEntries),
    operation,
  };
}

function cleanupPointerDrag() {
  pointerDragCleanup?.();
  pointerDragCleanup = null;
  pointerDrag = null;
  dragGhost.value = { ...dragGhost.value, visible: false, operation: 'auto' };
  document.body.classList.remove('is-file-pointer-dragging');
}

function handleFilePointerDragStart(payload) {
  const event = payload.event;

  if (event.button !== 0 || event.defaultPrevented) {
    return;
  }

  if (event.pointerType === 'mouse') {
    return;
  }

  if (!payload.entry?.path) {
    return;
  }

  cleanupPointerDrag();

  pointerDrag = {
    id: null,
    pointerId: event.pointerId,
    sourcePaneId: props.paneId,
    entries: [],
    payload,
    startX: event.clientX,
    startY: event.clientY,
    active: false,
  };

  const handleMove = (moveEvent) => {
    if (!pointerDrag || moveEvent.pointerId !== pointerDrag.pointerId) {
      return;
    }

    const deltaX = moveEvent.clientX - pointerDrag.startX;
    const deltaY = moveEvent.clientY - pointerDrag.startY;

    if (!pointerDrag.active && Math.hypot(deltaX, deltaY) >= POINTER_DRAG_THRESHOLD) {
      const entries = draggableEntriesFor(pointerDrag.payload);

      if (entries.length === 0) {
        cleanupPointerDrag();
        return;
      }

      pointerDrag.entries = entries;
      pointerDrag.active = true;
      cancelFileDragClear();
      store.startFileDrag(
        pointerDrag.sourcePaneId,
        pointerDrag.entries,
        forcedTransferModeFromEvent(moveEvent),
      );
      pointerDrag.id = store.dragOperation?.id || null;
      updateDragGhost(moveEvent, pointerDrag.entries);
      document.body.classList.add('is-file-pointer-dragging');
    }

    if (pointerDrag.active) {
      store.setFileDragMode(forcedTransferModeFromEvent(moveEvent));
      updateDragGhost(moveEvent, pointerDrag.entries);
      moveEvent.preventDefault();
      moveEvent.stopPropagation();
    }
  };

  const handleEnd = (endEvent) => {
    if (!pointerDrag || endEvent.pointerId !== pointerDrag.pointerId) {
      return;
    }

    const state = pointerDrag;
    const shouldDrop = state.active && state.entries.length > 0;
    cleanupPointerDrag();

    if (!shouldDrop) {
      return;
    }

    endEvent.preventDefault();
    endEvent.stopPropagation();

    finishPointerFileDrop(endEvent, state)
      .catch(async (error) => {
        console.error(error);
        await dialog.alert({
          title: 'Drag and Drop Failed',
          message: error?.message || 'The dragged items could not be dropped there.',
          variant: 'warning',
        });
      })
      .finally(() => {
        clearFileDragNow();
      });
  };

  const handleCancel = (cancelEvent) => {
    if (!pointerDrag || cancelEvent.pointerId !== pointerDrag.pointerId) {
      return;
    }

    cleanupPointerDrag();
    clearFileDragNow();
  };

  const abortPointerDrag = () => {
    cleanupPointerDrag();
    clearFileDragNow();
  };

  const handleKeyDown = (keyEvent) => {
    if (keyEvent.key === 'Escape') {
      abortPointerDrag();
    }
  };

  window.addEventListener('pointermove', handleMove, true);
  window.addEventListener('pointerup', handleEnd, true);
  window.addEventListener('pointercancel', handleCancel, true);
  window.addEventListener('blur', abortPointerDrag, true);
  window.addEventListener('keydown', handleKeyDown, true);

  pointerDragCleanup = () => {
    window.removeEventListener('pointermove', handleMove, true);
    window.removeEventListener('pointerup', handleEnd, true);
    window.removeEventListener('pointercancel', handleCancel, true);
    window.removeEventListener('blur', abortPointerDrag, true);
    window.removeEventListener('keydown', handleKeyDown, true);
  };
}

function effectiveDirectory(paneId = props.paneId) {
  return store.effectiveDirectoryFor(paneId) || store.activeTabFor(paneId)?.currentPath || '';
}

function parentDirectoriesForEntries(operationEntries) {
  return [...new Set(
    operationEntries
      .map((entry) => store.parentDirectoryFor(entry.path))
      .filter(Boolean),
  )];
}

async function refreshDirectories(paths, paneIds = null) {
  const uniquePaths = [...new Set(paths.filter(Boolean))];

  await Promise.all(uniquePaths.map((path) => store.reloadDirectoryInPanes(path, paneIds)));
}

function refreshPathsForOperationDirectory(directory) {
  return [directory, effectiveDirectory()].filter(Boolean);
}

function requestColumnRefreshes(paths, paneIds = null) {
  const uniquePaths = [...new Set(paths.filter(Boolean))];
  const targetPaneIds = Array.isArray(paneIds) && paneIds.length > 0
    ? paneIds
    : ['left', 'right'];

  for (const paneId of targetPaneIds) {
    for (const path of uniquePaths) {
      store.requestColumnDirectoryRefresh(paneId, path);
    }
  }
}

async function reloadPanesAfterMove(sourcePaneId, targetDirectory, operationEntries = []) {
  const paneIds = [...new Set([sourcePaneId, props.paneId].filter(Boolean))];
  const touchedDirectories = [
    targetDirectory,
    ...parentDirectoriesForEntries(operationEntries),
  ];

  await refreshDirectories(touchedDirectories, paneIds);
  requestColumnRefreshes(touchedDirectories, paneIds);
}

function handleFileDragStart(payload) {
  const entries = draggableEntriesFor(payload);

  if (entries.length === 0) {
    return;
  }

  writeDragPayload(payload.event, entries);
}

function handlePaneFileDragOver(event) {
  if (!isFileTransferDragEvent(event)) {
    return;
  }

  const paneDrop = paneDropFromEvent(event);

  if (!paneDrop) {
    return;
  }

  if (
    store.dragOperation?.entries?.length &&
    !shouldTransferToPaneDrop(store.dragOperation, paneDrop, event)
  ) {
    return;
  }

  event.preventDefault();

  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = dropEffectFromEvent(event);
  }
}

function handlePaneFileDrop(event) {
  if (!isFileTransferDragEvent(event)) {
    return;
  }

  const dragOperation = readDragPayload(event);
  const paneDrop = paneDropFromEvent(event);

  if (!dragOperation?.entries?.length || !paneDrop) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  transferEntriesToPointerDrop(event, dragOperation, paneDrop);
}

function handleFileDragEnd(event) {
  const dragOperation = store.dragOperation?.entries?.length
    ? {
        id: store.dragOperation.id,
        sourcePaneId: store.dragOperation.sourcePaneId,
        requestedMode: store.dragOperation.requestedMode,
        entries: store.dragOperation.entries,
      }
    : null;
  const elements = dragOperation && hasPointerCoordinates(event)
    ? pointerElements(event)
    : [];
  const favoriteDrop = elements.length > 0
    ? pointerFavoriteDrop(elements, event)
    : null;

  if (dragOperation && favoriteDrop) {
    finishPointerFileDrop(event, dragOperation)
      .catch(async (error) => {
        console.error(error);
        await dialog.alert({
          title: 'Drag and Drop Failed',
          message: error?.message || 'The dragged items could not be dropped there.',
          variant: 'warning',
        });
      })
      .finally(() => {
        clearFileDragNow();
      });
    return;
  }

  const paneDrop = dragOperation && hasPointerCoordinates(event)
    ? pointerPaneDrop(elements)
    : null;

  if (shouldTransferToPaneDrop(dragOperation, paneDrop, event)) {
    transferEntriesToPointerDrop(event, dragOperation, paneDrop);
    return;
  }

  scheduleFileDragClear();
}

async function handleFileDrop(targetEntry, event, currentTargetDirectory = null) {
  const payload = readDragPayload(event);
  const targetDirectory = targetEntry?.kind === 'directory'
    ? targetEntry.path
    : currentTargetDirectory || effectiveDirectory();

  if (!payload?.entries?.length || !targetDirectory) {
    clearFileDragNow();
    return;
  }

  if (!shouldTransferToPaneDrop(payload, { targetPaneId: props.paneId, targetDirectory }, event)) {
    clearFileDragNow();
    return;
  }

  if (!store.claimFileDrop(payload.id)) {
    return;
  }

  clearFileDragNow();

  try {
    const mode = await transfers.transferModeForEvent(
      event,
      payload.entries,
      targetDirectory,
    );
    const transferred = await transfers.transferEntries({
      mode,
      entries: payload.entries,
      targetDirectory,
    });

    if (transferred) {
      await reloadPanesAfterMove(payload.sourcePaneId, targetDirectory, payload.entries);
    }
  } catch (error) {
    console.error(error);
    await dialog.alert({
      title: 'Transfer Failed',
      message: error?.message || 'The selected items could not be transferred.',
      variant: 'warning',
    });
  } finally {
    clearFileDragNow();
  }
}

function contextOperationEntries(menu) {
  if (!menu?.entry) {
    return [];
  }

  if (Array.isArray(menu.operationEntries) && menu.operationEntries.length > 0) {
    return menu.operationEntries;
  }

  if (Number.isInteger(menu.index) && store.isEntrySelected(props.paneId, menu.index)) {
    const selectedEntries = store.selectedEntriesFor(props.paneId);

    if (selectedEntries.length > 0) {
      return selectedEntries;
    }
  }

  return [menu.entry];
}

async function copyPathToClipboard(path) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(path);
  }
}

function isLocalEntry(entry) {
  return Boolean(entry?.path) && !isArchivePath(entry.path);
}

function isExtractableArchiveEntry(entry) {
  return entry?.kind === 'file' && !isArchivePath(entry.path) && /\.(zip|tar|tar\.gz|tgz|tar\.zst|tzst|7z)$/i.test(entry.name || '');
}

function extensionListForTool(tool) {
  return String(tool?.extensions || '')
    .split(/[,\s]+/)
    .map((extension) => extension.trim().replace(/^\.+/, '').toLowerCase())
    .filter(Boolean);
}

function customToolAppliesToEntry(tool, entry) {
  const appliesTo = tool?.appliesTo || 'both';

  if (entry?.kind === 'directory') {
    return appliesTo === 'both' || appliesTo === 'folders';
  }

  if (entry?.kind !== 'file' || appliesTo === 'folders') {
    return false;
  }

  if (appliesTo !== 'files') {
    return true;
  }

  const extensions = extensionListForTool(tool);

  if (extensions.length === 0) {
    return true;
  }

  return extensions.includes(extensionForName(entry.name));
}

function pathJoin(directory, name) {
  const base = String(directory || '').replace(/\/+$/, '');

  if (!base || base === '/') {
    return `/${name}`;
  }

  return `${base}/${name}`;
}

function commonParentDirectoryFor(operationEntries) {
  const parents = parentDirectoriesForEntries(operationEntries);

  return parents.length === 1 ? parents[0] : effectiveDirectory();
}

async function existingNamesInDirectory(directory) {
  const directoryEntries = await listDirectory(directory);

  return new Set(directoryEntries.map((item) => item.name.toLocaleLowerCase()));
}

async function runQueuedArchive({ paths, destination, options, overwrite, label, refreshPaths, successDetail }) {
  const retryAction = () => runQueuedArchive({
    paths,
    destination,
    options,
    overwrite: true,
    label,
    refreshPaths,
    successDetail,
  });
  const jobId = store.startQueueJob({
    operation: 'archive',
    label,
    remotePaths: [destination, ...paths],
    retryAction,
  });

  try {
    await archiveItems(paths, destination, options, overwrite, jobId);
    await refreshDirectories(refreshPaths);
    store.completeQueueJob(jobId, successDetail);
  } catch (error) {
    if (error?.code === 'operation_cancelled') {
      store.cancelQueueJobDone(jobId);
      return;
    }

    store.failQueueJob(jobId, error?.message || 'Archive creation failed.', {
      failedItems: paths.map((path) => ({
        path,
        message: error?.message || 'Failed',
      })),
    });
    throw error;
  }
}

async function runQueuedUnarchive({ paths, destinationDirectory, label, refreshPaths }) {
  const retryAction = () => runQueuedUnarchive({
    paths,
    destinationDirectory,
    label,
    refreshPaths,
  });
  const jobId = store.startQueueJob({
    operation: 'unarchive',
    label,
    remotePaths: [destinationDirectory, ...paths],
    retryAction,
  });

  try {
    await unarchiveItems(paths, destinationDirectory, jobId);
    await refreshDirectories(refreshPaths);
    store.completeQueueJob(jobId, 'Archive extracted');
  } catch (error) {
    if (error?.code === 'operation_cancelled') {
      store.cancelQueueJobDone(jobId);
      return;
    }

    store.failQueueJob(jobId, error?.message || 'Archive extraction failed.', {
      failedItems: paths.map((path) => ({
        path,
        message: error?.message || 'Failed',
      })),
    });
    throw error;
  }
}

async function createArchive(menu) {
  const operationEntries = contextOperationEntries(menu);
  const currentPath = commonParentDirectoryFor(operationEntries);

  if (!currentPath || operationEntries.length === 0 || operationEntries.some((item) => !isLocalEntry(item))) {
    await dialog.alert({
      title: 'Archive Not Available',
      message: 'Archives can only be created from normal files and folders.',
      variant: 'warning',
    });
    return;
  }

  const existingNames = await existingNamesInDirectory(currentPath);
  archiveDialog.value = {
    visible: true,
    entries: operationEntries,
    directory: currentPath,
    existingNames: Array.from(existingNames),
  };
}

function closeArchiveDialog() {
  archiveDialog.value = {
    ...archiveDialog.value,
    visible: false,
  };
}

async function handleCreateArchive(payload) {
  const { entries: operationEntries, directory: currentPath } = archiveDialog.value;
  const archiveName = String(payload?.archiveName || '').trim();

  if (!currentPath || operationEntries.length === 0 || !archiveName || /[\\/]/.test(archiveName)) {
    await dialog.alert({
      title: 'Invalid Archive Name',
      message: 'Archive names cannot be empty or contain folder separators.',
      variant: 'warning',
    });
    return;
  }

  const destination = pathJoin(currentPath, archiveName);

  if (operationEntries.some((item) => item.path === destination)) {
    await dialog.alert({
      title: 'Archive Name Conflict',
      message: 'Choose a different archive name.',
      detail: 'The archive destination cannot be one of the selected files.',
      variant: 'warning',
    });
    return;
  }

  closeArchiveDialog();

  try {
    await runQueuedArchive({
      paths: operationEntries.map((item) => item.path),
      destination,
      options: {
        format: payload.format,
        compressionLevel: payload.compressionLevel,
        includeTopLevelDirectory: payload.includeTopLevelDirectory,
        password: payload.password || null,
      },
      overwrite: Boolean(payload.overwrite),
      label: `Creating ${archiveName}`,
      refreshPaths: refreshPathsForOperationDirectory(currentPath),
      successDetail: `"${archiveName}" created`,
    });
  } catch (error) {
    console.error(error);
    await dialog.alert({
      title: 'Archive Failed',
      message: error?.message || 'The archive could not be created.',
      variant: 'warning',
    });
  }
}

async function extractArchive(menu) {
  const operationEntries = contextOperationEntries(menu);
  const currentPath = commonParentDirectoryFor(operationEntries);

  if (!currentPath || operationEntries.length === 0 || operationEntries.some((item) => !isLocalEntry(item) || !isExtractableArchiveEntry(item))) {
    await dialog.alert({
      title: 'Extract Not Available',
      message: 'Only supported archives in file panels can be extracted.',
      variant: 'warning',
    });
    return;
  }

  await runQueuedUnarchive({
    paths: operationEntries.map((item) => item.path),
    destinationDirectory: currentPath,
    label: operationEntries.length === 1
      ? `Extracting ${operationEntries[0].name}`
      : `Extracting ${operationEntries.length} archives`,
    refreshPaths: refreshPathsForOperationDirectory(currentPath),
  });
}

async function runContextCustomTool(menu, toolId) {
  const tool = configuredCustomTools.value.find((candidate) => candidate.id === toolId);
  const operationEntries = contextOperationEntries(menu);

  if (
    !tool ||
    operationEntries.length === 0 ||
    operationEntries.some((item) => !isLocalEntry(item)) ||
    !operationEntries.every((item) => customToolAppliesToEntry(tool, item))
  ) {
    await dialog.alert({
      title: 'Tool Not Available',
      message: 'This tool is not available for the selected items.',
      variant: 'warning',
    });
    return;
  }

  await runCustomTool(
    tool.command,
    operationEntries.map((item) => item.path),
    commonParentDirectoryFor(operationEntries),
  );
}

async function openEntryAt(index) {
  const entry = entries.value[index];

  if (!entry) {
    return;
  }

  await openEntryPayload(entry, index);
}

async function openEntryPayload(entry, index = null) {
  if (!entry) {
    return;
  }

  try {
    if (entry.kind === 'directory') {
      if (Number.isInteger(index)) {
        store.openEntry(props.paneId, index);
      } else {
        store.setPanePath(props.paneId, entry.path);
      }
    } else if (isArchiveEntry(entry)) {
      store.setPanePath(props.paneId, archiveRootPath(entry.path));
    } else {
      await openWithDefaultApp(entry.path);
    }
  } catch (error) {
    console.error(error);
    await dialog.alert({
      title: 'Unable to Open Item',
      message: error?.message || 'The selected item could not be opened.',
      variant: 'warning',
    });
  }
}

async function showOpenWithDialog(menu) {
  const operationEntries = contextOperationEntries(menu);
  const entry = operationEntries[0];
  const requestId = openWithRequestId + 1;

  if (operationEntries.length !== 1 || !entry || entry.kind !== 'file' || !isLocalEntry(entry)) {
    await dialog.alert({
      title: 'Open With Not Available',
      message: 'Choose one file to open with another app.',
      variant: 'warning',
    });
    return;
  }

  openWithRequestId = requestId;
  openWithDialog.value = {
    visible: true,
    entry,
    context: null,
    loading: true,
    error: '',
  };

  try {
    const context = await listOpenWithApps(entry.path);

    if (requestId !== openWithRequestId || !openWithDialog.value.visible) {
      return;
    }

    openWithDialog.value = {
      visible: true,
      entry,
      context,
      loading: false,
      error: '',
    };
  } catch (error) {
    console.error(error);

    if (requestId !== openWithRequestId || !openWithDialog.value.visible) {
      return;
    }

    openWithDialog.value = {
      visible: true,
      entry,
      context: null,
      loading: false,
      error: error?.message || 'Unable to load apps for this file.',
    };
  }
}

function closeOpenWithDialog() {
  openWithRequestId += 1;
  openWithDialog.value = {
    visible: false,
    entry: null,
    context: null,
    loading: false,
    error: '',
  };
}

async function handleOpenWithSelection(payload) {
  const entry = openWithDialog.value.entry;

  if (!entry || !payload?.appId) {
    return;
  }

  try {
    await openWithApp(entry.path, payload.appId, Boolean(payload.remember));
    closeOpenWithDialog();
  } catch (error) {
    console.error(error);
    await dialog.alert({
      title: 'Unable to Open Item',
      message: error?.message || 'The selected app could not open this file.',
      variant: 'warning',
    });
  }
}

async function revealOpenWithEntry() {
  const entry = openWithDialog.value.entry;

  if (!entry) {
    return;
  }

  try {
    await revealInFileManager(entry.path);
  } catch (error) {
    console.error(error);
    await dialog.alert({
      title: 'Unable to Reveal Item',
      message: error?.message || 'The selected item could not be revealed.',
      variant: 'warning',
    });
  }
}

function openSelectedEntry() {
  openEntryAt(activeTab.value.selectedIndex);
}

async function handleContextAction(action) {
  const menu = contextMenu.value;
  const entry = menu?.entry;

  closeContextMenu();

  if (!entry) {
    return;
  }

  try {
    if (action === 'open') {
      if (entry.kind === 'directory') {
        if (Number.isInteger(menu.index)) {
          store.openEntry(props.paneId, menu.index);
        } else {
          store.setPanePath(props.paneId, entry.path);
        }
      } else if (isArchiveEntry(entry)) {
        store.setPanePath(props.paneId, archiveRootPath(entry.path));
      } else {
        await openWithDefaultApp(entry.path);
      }
      return;
    }

    if (action === 'openInNewTab') {
      if (entry.kind === 'directory' || isArchiveEntry(entry)) {
        store.addPaneTab(props.paneId, entry.kind === 'directory' ? entry.path : archiveRootPath(entry.path));
      }
      return;
    }

    if (action === 'openWith') {
      await showOpenWithDialog(menu);
      return;
    }

    if (action === 'editFile') {
      await editFile(entry.path, store.appSettings.editorCommand);
      return;
    }

    if (action === 'reveal') {
      await revealInFileManager(entry.path);
      return;
    }

    if (action === 'copyPath') {
      await copyPathToClipboard(contextOperationEntries(menu).map((item) => item.path).join('\n'));
      return;
    }

    if (typeof action === 'string' && action.startsWith('customTool:')) {
      await runContextCustomTool(menu, action.slice('customTool:'.length));
      return;
    }

    if (action === 'rename') {
      const nextName = (await dialog.prompt({
        title: 'Rename Item',
        message: entry.name,
        inputLabel: 'Name',
        inputValue: entry.name,
        confirmLabel: 'Rename',
      }))?.trim();

      if (nextName && nextName !== entry.name) {
        const renamed = await transfers.renameEntry(entry, nextName);

        if (!renamed) {
          return;
        }

        await refreshDirectories([store.parentDirectoryFor(entry.path)]);
      }
      return;
    }

    if (action === 'archive') {
      await createArchive(menu);
      return;
    }

    if (action === 'unarchive') {
      await extractArchive(menu);
      return;
    }

    if (action === 'delete') {
      const deleteEntries = contextOperationEntries(menu);
      const label = deleteEntries.length === 1 ? `"${deleteEntries[0].name}"` : `${deleteEntries.length} items`;
      const confirmed = store.appSettings.confirmDelete
        ? await dialog.confirm({
            title: 'Delete Item',
            message: `Delete ${label}?`,
            detail: 'This cannot be undone from inside the app.',
            confirmLabel: 'Delete',
            variant: 'danger',
            destructive: true,
          })
        : true;

      if (confirmed) {
        const touchedDirectories = parentDirectoriesForEntries(deleteEntries);
        await deleteItems(deleteEntries.map((item) => item.path));
        store.clearSelection(props.paneId);
        await refreshDirectories(touchedDirectories);
      }
      return;
    }

    if (action === 'copyToOtherPane' || action === 'moveToOtherPane') {
      const targetPane = store.activeTabFor(otherPaneId.value);

      if (!targetPane) {
        return;
      }

      const transferEntries = contextOperationEntries(menu);
      const targetDirectory = effectiveDirectory(otherPaneId.value);
      const transferred = action === 'copyToOtherPane'
        ? await transfers.copyEntries({
            entries: transferEntries,
            targetDirectory,
          })
        : await transfers.moveEntries({
            entries: transferEntries,
            targetDirectory,
          });

      if (!transferred) {
        return;
      }

      const touchedDirectories = action === 'copyToOtherPane'
        ? [targetDirectory]
        : [targetDirectory, ...parentDirectoriesForEntries(transferEntries)];
      await refreshDirectories(touchedDirectories);
    }
  } catch (error) {
    console.error(error);
    await dialog.alert({
      title: 'File Operation Failed',
      message: error?.message || 'The file operation could not be completed.',
      variant: 'warning',
    });
  }
}
</script>

<template>
  <section
    class="pane"
    :class="{ 'pane--active': isActive }"
    :data-pane-id="paneId"
    :aria-label="`${title} file pane`"
    tabindex="0"
    @focusin="store.setActivePane(paneId)"
    @click="store.setActivePane(paneId)"
    @dragover.capture="handlePaneFileDragOver"
    @dragover="handlePaneFileDragOver"
    @drop="handlePaneFileDrop"
    @keydown.up.prevent.stop="store.moveSelection(paneId, -1, { extend: $event.shiftKey })"
    @keydown.down.prevent.stop="store.moveSelection(paneId, 1, { extend: $event.shiftKey })"
    @keydown.enter.prevent.stop="openSelectedEntry"
    @keydown.backspace.prevent.stop="store.goToParent(paneId)"
  >
    <nav
      class="pane-tabs"
      :class="{ 'pane-tabs--tab-drop': tabDropIndex !== null }"
      :aria-label="`${title} tabs`"
      @dragover="handleTabStripDragOver"
      @dragleave="handleTabStripDragLeave"
      @drop="handleTabStripDrop"
    >
      <div
        v-for="(tab, tabIndex) in pane.tabs"
        :key="tab.id"
        class="pane-tab"
        :class="{
          'pane-tab--active': tab.id === pane.activeTabId,
          'pane-tab--dragging': draggedTabId === tab.id,
          'pane-tab--drop-before': tabDropIndex === tabIndex,
        }"
        draggable="true"
        @dragstart.stop="handleTabDragStart(tab, $event)"
        @dragend.stop="handleTabDragEnd"
        @dragover="handleTabDragOver(tab, $event)"
        @drop="handleTabDrop(tab, $event)"
        @contextmenu="showTabContextMenu(tab, $event)"
      >
        <button
          type="button"
          class="tab-select"
          :title="tab.currentPath"
          draggable="false"
          @click.stop="activateTab(tab.id)"
        >
          <AppIcon name="folder" :size="15" :stroke-width="1.7" class="tab-icon" />
          <span class="tab-label">{{ store.tabTitle(tab) }}</span>
        </button>
        <span v-if="tab.loading" class="tab-activity" aria-hidden="true"></span>
        <button
          v-else-if="pane.tabs.length > 1"
          type="button"
          class="tab-close"
          :aria-label="`Close ${store.tabTitle(tab)} tab`"
          draggable="false"
          @click.stop="closeTab(tab.id)"
        >
          <AppIcon name="x" :size="12" :stroke-width="2.2" />
        </button>
      </div>

      <button
        type="button"
        class="tab-add"
        :class="{ 'tab-add--drop-before': tabDropIndex === pane.tabs.length }"
        aria-label="New tab"
        @dragover="handleTabStripDragOver"
        @drop="handleTabStripDrop"
        @click.stop="addTab"
      >
        <AppIcon name="plus" :size="14" :stroke-width="2.1" />
      </button>
    </nav>

    <header class="pane-header">
      <nav class="breadcrumbs" aria-label="Current path">
        <AppIcon name="folder" :size="13" :stroke-width="1.9" class="breadcrumbs-icon" />
        <button
          v-for="(crumb, index) in breadcrumbs"
          :key="`${crumb.path}-${index}`"
          type="button"
          :title="crumb.path"
          @click.stop="navigateToBreadcrumb(crumb.path)"
          @keydown.stop
        >
          {{ crumb.label }}
        </button>
      </nav>

      <div class="pane-header-bottom">
        <p class="pane-summary">
          <span
            v-for="(part, index) in summaryParts"
            :key="part"
            class="pane-summary-part"
            :class="{ 'pane-summary-part--primary': index === 0 }"
          >
            {{ part }}
          </span>
        </p>

        <div class="pane-meta">
          <span v-if="activeTab.loading">Loading</span>
          <template v-else>
            <template v-if="activeTab.viewMode === 'grid' || activeTab.viewMode === 'columns'">
              <div class="sort-combo" role="group" aria-label="Sort options">
                <label class="sort-control">
                  <span class="visually-hidden">Sort by</span>
                  <select :value="activeTab.sortKey" @change.stop="setSortKey" @keydown.stop>
                    <option value="name">Name</option>
                    <option value="extension">Extension</option>
                    <option value="size">Size</option>
                    <option value="modifiedAt">Modified</option>
                    <option value="none">Unsorted</option>
                  </select>
                </label>
                <button
                  type="button"
                  class="sort-direction"
                  :aria-label="`Sort ${sortDirectionLabel.toLowerCase()}`"
                  :title="sortDirectionLabel"
                  @click.stop="toggleSortDirection"
                  @keydown.stop
                >
                  <AppIcon
                    name="chevron-down"
                    :size="12"
                    :stroke-width="2.1"
                    :class="{ 'sort-direction-icon--asc': activeTab.sortDirection === 'asc' }"
                  />
                </button>
              </div>
            </template>
          </template>
        </div>
      </div>
    </header>

    <div class="pane-body">
      <p v-if="activeTab.error" class="pane-error">{{ activeTab.error }}</p>

      <div class="pane-list-host">
        <FileList
          :pane-id="paneId"
          :entries="entries"
          :raw-entry-count="rawEntryCount"
          :search-query="activeSearchQuery"
          :selected-index="activeTab.selectedIndex"
          :selected-paths="activeTab.selectedPaths"
          :loading="activeTab.loading"
          :loaded="activeTab.loaded"
          :view-mode="activeTab.viewMode"
          :directory-key="activeTab.currentPath"
          :sort-key="activeTab.sortKey"
          :sort-direction="activeTab.sortDirection"
          :is-entry-selected="(index) => store.isEntrySelected(paneId, index)"
          :show-hidden-files="store.showHiddenFiles"
          :date-format="store.appSettings.dateFormat"
          :refresh-key="activeTab.loadVersion"
          :column-refresh-request="store.columnRefreshRequests[paneId]"
          :column-selection-reset-key="store.columnSelectionResetKeys[paneId]"
          :dragging="isFileDragActive"
          :dragged-paths="draggedPaths"
          @select="handleFileSelect"
          @open="openEntryAt"
          @open-parent="store.setPanePath(paneId, $event)"
          @open-path="openEntryPayload"
          @preview-entry="store.setColumnPreviewEntry(paneId, $event)"
          @active-directory="store.setColumnTargetDirectory(paneId, $event)"
          @column-summary="updateColumnSummary"
          @drag-start="handleFileDragStart"
          @drag-end="handleFileDragEnd"
          @pointer-drag-start="handleFilePointerDragStart"
          @drop-entry="handleFileDrop($event.entry, $event.event)"
          @drop-current="handleFileDrop(null, $event.event, $event.targetDirectory)"
          @background-click="handleBackgroundClick"
          @sort="store.setPaneSort(paneId, $event)"
          @context="showContextMenu"
        />
      </div>
    </div>

    <FileContextMenu
      v-if="contextMenu"
      :entry="contextMenu.entry"
      :position="contextMenu.position"
      :can-archive="canArchiveContext"
      :can-unarchive="canUnarchiveContext"
      :can-open-with="canOpenWithContext"
      :can-edit-file="canEditFileContext"
      :can-custom-tools="canRunCustomToolContext"
      :custom-tools="availableCustomTools"
      :can-transfer="canTransferToOtherPane"
      :can-modify="canModifyContext"
      :can-move="canMoveContext"
      @action="handleContextAction"
      @close="closeContextMenu"
    />

    <TabContextMenu
      v-if="tabContextMenu && tabForId(tabContextMenu.tabId)"
      :tab="tabForId(tabContextMenu.tabId)"
      :title="store.tabTitle(tabForId(tabContextMenu.tabId))"
      :position="tabContextMenu.position"
      :can-close="pane.tabs.length > 1"
      :can-close-others="pane.tabs.length > 1"
      @action="handleTabContextAction"
      @close="closeTabContextMenu"
    />

    <CreateArchiveDialog
      v-if="archiveDialog.visible"
      :visible="archiveDialog.visible"
      :entries="archiveDialog.entries"
      :directory="archiveDialog.directory"
      :existing-names="archiveDialog.existingNames"
      @cancel="closeArchiveDialog"
      @create="handleCreateArchive"
    />

    <OpenWithDialog
      v-if="openWithDialog.visible"
      :visible="openWithDialog.visible"
      :entry="openWithDialog.entry"
      :context="openWithDialog.context"
      :loading="openWithDialog.loading"
      :error="openWithDialog.error"
      @cancel="closeOpenWithDialog"
      @open="handleOpenWithSelection"
      @reveal="revealOpenWithEntry"
    />
  </section>

  <Teleport to="body">
    <div
      v-if="dragGhost.visible"
      class="file-drag-ghost"
      :class="`file-drag-ghost--${dragGhost.kind}`"
      :style="{ transform: `translate3d(${dragGhost.x}px, ${dragGhost.y}px, 0)` }"
      aria-hidden="true"
    >
      <span class="file-drag-ghost-icon">
        <AppIcon :name="ghostIconName(dragGhost.kind)" :size="17" :stroke-width="1.9" />
      </span>
      <span class="file-drag-ghost-operation">{{ ghostOperationLabel(dragGhost.operation) }}</span>
      <span class="file-drag-ghost-label">{{ dragGhost.label }}</span>
      <span v-if="dragGhost.count > 1" class="file-drag-ghost-count">{{ dragGhost.count }}</span>
    </div>
  </Teleport>
</template>

<style scoped>
.pane {
  position: relative;
  display: grid;
  grid-template-rows: 55px 56px minmax(0, 1fr);
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  border-radius: 0;
  background: var(--pane-glass);
}

.file-drag-ghost {
  position: fixed;
  top: 0;
  left: 0;
  z-index: 10000;
  display: grid;
  grid-template-columns: 15px auto minmax(0, 1fr) auto;
  max-width: min(232px, calc(100vw - 28px));
  height: 27px;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
  border: 1px solid color-mix(in srgb, var(--control-border) 70%, transparent);
  border-radius: 6px;
  background: color-mix(in srgb, var(--popover-bg) 78%, transparent);
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.04),
    0 1px 3px rgb(0 0 0 / 0.14);
  color: var(--text-muted);
  opacity: 0.84;
  pointer-events: none;
  contain: layout paint style;
  user-select: none;
  will-change: transform;
  animation: file-drag-ghost-in 110ms cubic-bezier(0.25, 1, 0.5, 1);
}

.file-drag-ghost-icon {
  display: grid;
  width: 15px;
  height: 15px;
  place-items: center;
  color: color-mix(in srgb, var(--file-icon) 78%, var(--text-faint));
}

.file-drag-ghost--directory .file-drag-ghost-icon {
  color: color-mix(in srgb, var(--folder-icon) 78%, var(--text-faint));
}

.file-drag-ghost--multiple .file-drag-ghost-icon {
  color: color-mix(in srgb, var(--accent) 72%, var(--text-faint));
}

.file-drag-ghost-label {
  overflow: hidden;
  min-width: 0;
  font-size: 11.5px;
  font-weight: 580;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-drag-ghost-operation {
  color: var(--text-faint);
  font-size: 9.5px;
  font-weight: 640;
  letter-spacing: 0;
}

.file-drag-ghost-count {
  display: grid;
  min-width: 16px;
  height: 16px;
  place-items: center;
  border-radius: 999px;
  padding: 0 4px;
  background: color-mix(in srgb, var(--control-bg) 68%, transparent);
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 650;
  line-height: 1;
}

@keyframes file-drag-ghost-in {
  from { opacity: 0; }
  to { opacity: 0.84; }
}

@media (prefers-reduced-motion: reduce) {
  .file-drag-ghost {
    animation: none;
  }
}

/* Active pane — subtle blue top edge */
.pane::before {
  display: none;
}

.pane--active .pane-tabs {
  box-shadow: inset 0 -1px 0 var(--accent);
}

/* ── Tabs ────────────────────────────────────────────────── */
.pane-tabs {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 8px;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
  min-height: 46px;
  padding: 4px 14px 6px;
  background: color-mix(in srgb, var(--pane-glass) 78%, var(--desktop-bg));
  border-bottom: 1px solid var(--separator);
}

.pane-tabs::-webkit-scrollbar { display: none; }

.pane-tabs--tab-drop {
  box-shadow: inset 0 -1px 0 var(--accent);
}

.pane-tab {
  position: relative;
  display: flex;
  flex: 0 1 180px;
  max-width: 180px;
  min-width: 116px;
  height: 38px;
  align-items: center;
  gap: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--text-faint);
  cursor: grab;
  text-align: left;
  user-select: none;
  transition:
    background 110ms ease,
    color 110ms ease,
    opacity 110ms ease,
    transform 110ms ease;
}

.pane-tab:active {
  cursor: grabbing;
}

.pane-tab--dragging {
  opacity: 0.46;
  transform: scale(0.985);
}

.pane-tab--drop-before::before,
.tab-add--drop-before::before {
  content: '';
  position: absolute;
  z-index: 2;
  top: 6px;
  bottom: 6px;
  left: -5px;
  width: 2px;
  border-radius: 999px;
  background: var(--accent);
  box-shadow:
    0 0 0 2px var(--accent-glow),
    0 0 12px var(--accent-glow);
}

.tab-select {
  display: flex;
  align-items: center;
  gap: 9px;
  overflow: hidden;
  flex: 1;
  min-width: 0;
  height: 100%;
  padding: 0 11px;
  background: transparent;
  color: inherit;
  text-align: left;
}

.tab-icon {
  flex-shrink: 0;
  color: inherit;
}

.pane-tab:hover {
  background: rgb(255 255 255 / 0.07);
  color: var(--text-muted);
}

.pane-tab--active {
  background: rgb(0 0 0 / 0.24);
  color: var(--text);
}

@media (prefers-color-scheme: light) {
  .pane-tab:hover {
    background: rgb(0 0 0 / 0.05);
  }

  .pane-tab--active {
    background: rgb(0 0 0 / 0.09);
  }
}


.tab-label {
  overflow: hidden;
  flex: 1;
  min-width: 0;
  font-size: 12px;
  font-weight: 670;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Close button */
.tab-close {
  display: inline-grid;
  flex-shrink: 0;
  width: 18px;
  height: 18px;
  margin-right: 6px;
  place-items: center;
  border-radius: 50%;
  background: transparent;
  color: var(--text-faint);
  opacity: 0;
  transition:
    background 100ms ease,
    color 100ms ease,
    opacity 100ms ease;
}

.pane-tab:hover .tab-close,
.pane-tab--active .tab-close {
  opacity: 1;
}

.tab-close:hover {
  color: var(--text);
}

/* Loading dot */
.tab-activity {
  flex-shrink: 0;
  width: 5px;
  height: 5px;
  margin-right: 6px;
  border-radius: 50%;
  background: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-glow);
}

/* Add button */
.tab-add {
  position: relative;
  display: grid;
  width: 30px;
  height: 34px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 6px;
  margin-left: 2px;
  background: transparent;
  color: var(--text-faint);
  transition:
    background 100ms ease,
    color 100ms ease;
}

.tab-add:hover {
  background: var(--btn-hover);
  color: var(--text-muted);
}

/* ── Pane header ──────────────────────────────────────────── */
.pane-header {
  display: grid;
  grid-template-rows: auto auto;
  gap: 2px;
  min-height: 56px;
  padding: 5px 16px;
  background: transparent;
  border-bottom: 1px solid var(--hairline);
}

.breadcrumbs {
  display: flex;
  width: 100%;
  min-width: 0;
  align-items: center;
  gap: 4px;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}

.breadcrumbs::-webkit-scrollbar {
  display: none;
}

.breadcrumbs-icon {
  flex: 0 0 auto;
  color: var(--text-faint);
}

.breadcrumbs button {
  position: relative;
  display: inline-flex;
  flex: 0 1 auto;
  align-items: center;
  justify-content: flex-start;
  overflow: hidden;
  max-width: min(190px, 34vw);
  height: 20px;
  min-width: 0;
  border-radius: 5px;
  padding: 0 6px;
  background: transparent;
  color: var(--text-faint);
  font-size: 12px;
  font-weight: 610;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition:
    background 90ms ease,
    color 90ms ease;
}

.breadcrumbs button + button {
  margin-left: 2px;
  padding-left: 14px;
}

.breadcrumbs button:last-child {
  color: var(--text-muted);
  font-weight: 670;
}

.breadcrumbs button:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.breadcrumbs button:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: -1px;
}

.breadcrumbs button + button::before {
  position: absolute;
  left: 3px;
  color: var(--text-faint);
  content: "›";
  font-size: 12px;
  line-height: 1;
  top: 50%;
  transform: translateY(-50%);
}

.pane-header-bottom {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.pane-summary {
  display: flex;
  align-items: center;
  min-width: 0;
  overflow: hidden;
  margin: 0;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 570;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pane-summary-part {
  display: inline-flex;
  min-width: 0;
  flex: 0 1 auto;
  align-items: center;
  overflow: hidden;
  text-overflow: ellipsis;
}

.pane-summary-part--primary {
  color: var(--text-muted);
  font-weight: 650;
}

.pane-summary-part + .pane-summary-part::before {
  content: "";
  width: 3px;
  height: 3px;
  flex: 0 0 auto;
  margin: 0 7px;
  border-radius: 50%;
  background: currentColor;
  opacity: 0.55;
}

.pane-meta {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  color: var(--text-faint);
  font-size: 12px;
  font-weight: 570;
  letter-spacing: 0;
}

.pane--active .pane-meta {
  color: color-mix(in srgb, var(--accent) 45%, var(--text-faint));
}

.sort-combo {
  display: inline-flex;
  height: 24px;
  min-width: 0;
  align-items: center;
  overflow: hidden;
  border: 1px solid var(--input-border);
  border-radius: 6px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--icon);
  transition: border-color 140ms ease, box-shadow 140ms ease;
}

.sort-combo:focus-within {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.sort-control {
  position: relative;
  display: inline-flex;
  height: 100%;
  min-width: 0;
  align-items: center;
}

.sort-control select {
  -webkit-appearance: none;
  appearance: none;
  width: 96px;
  height: 100%;
  min-width: 0;
  border: 0;
  border-radius: 0;
  padding: 0 7px;
  background: transparent;
  box-shadow: none;
  color: var(--text-muted);
  font: inherit;
  cursor: pointer;
  outline: 0;
  transition: color 100ms ease;
}

.sort-control select:hover {
  color: var(--text);
}

.sort-control select:focus-visible {
  outline: 0;
}

.sort-direction {
  display: inline-grid;
  width: 24px;
  height: 100%;
  place-items: center;
  border: 0;
  border-left: 1px solid var(--input-border);
  border-radius: 0;
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition:
    background 80ms ease,
    color 80ms ease;
}

.sort-direction:hover {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.sort-direction:focus-visible {
  outline: 0;
}

.sort-direction :deep(.sort-direction-icon--asc) {
  transform: rotate(180deg);
}


.visually-hidden {
  position: absolute;
  overflow: hidden;
  width: 1px;
  height: 1px;
  clip: rect(0 0 0 0);
  white-space: nowrap;
}

.pane-body {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.pane-list-host {
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
}

/* ── Error ────────────────────────────────────────────────── */
.pane-error {
  flex: 0 0 auto;
  margin: 0;
  border-bottom: 1px solid color-mix(in srgb, var(--danger) 38%, var(--hairline));
  padding: 8px 12px;
  background: color-mix(in srgb, var(--danger) 10%, transparent);
  color: var(--danger);
  font-size: 13px;
}
</style>
