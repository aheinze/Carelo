<script setup>
import { computed, nextTick, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import FileRow from './FileRow.vue';
import { listDirectory } from '../composables/useFileOperations';
import { dropEffectFromEvent } from '../composables/useFileTransferGuards';
import { archiveParentPath, archiveRootPath, isArchiveEntry, isArchivePath, isBrowsableEntry } from '../utils/archivePaths';

const NAME_COLLATOR = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
const SORT_KEYS = ['name', 'extension', 'size', 'modifiedAt', 'none'];

const props = defineProps({
  entries: {
    type: Array,
    required: true,
  },
  rawEntryCount: {
    type: Number,
    default: 0,
  },
  searchQuery: {
    type: String,
    default: '',
  },
  selectedIndex: {
    type: Number,
    required: true,
  },
  loading: {
    type: Boolean,
    required: true,
  },
  viewMode: {
    type: String,
    required: true,
  },
  directoryKey: {
    type: String,
    required: true,
  },
  sortKey: {
    type: String,
    required: true,
  },
  sortDirection: {
    type: String,
    required: true,
  },
  isEntrySelected: {
    type: Function,
    required: true,
  },
  showHiddenFiles: {
    type: Boolean,
    default: false,
  },
  dragging: {
    type: Boolean,
    default: false,
  },
  draggedPaths: {
    type: Array,
    default: () => [],
  },
  refreshKey: {
    type: [Number, String],
    default: 0,
  },
  columnRefreshRequest: {
    type: Object,
    default: null,
  },
  dateFormat: {
    type: String,
    default: 'system',
  },
});

const emit = defineEmits([
  'select',
  'open',
  'open-parent',
  'open-path',
  'sort',
  'context',
  'preview-entry',
  'active-directory',
  'column-summary',
  'drag-start',
  'drag-end',
  'pointer-drag-start',
  'drop-entry',
  'drop-current',
  'background-click',
]);

const listScroller = ref(null);
const gridScroller = ref(null);
const columnScroller = ref(null);
const columnTrail = ref([]);
const entryDropPath = ref('');
const currentDirectoryDropActive = ref(false);
const currentDirectoryDropPath = ref('');
let columnLoadVersion = 0;

const activeSearchQuery = computed(() => props.searchQuery.trim().toLowerCase());
const isSearchFiltering = computed(() => activeSearchQuery.value.length > 0);
const hasFilteredOutEntries = computed(() =>
  isSearchFiltering.value && props.rawEntryCount > 0 && props.entries.length === 0,
);
const hasHiddenOnlyEntries = computed(() =>
  !isSearchFiltering.value && !props.showHiddenFiles && props.rawEntryCount > 0 && props.entries.length === 0,
);
const emptyDirectoryMessage = computed(() =>
  hasFilteredOutEntries.value
    ? 'No items match your search.'
    : hasHiddenOnlyEntries.value
      ? 'Only hidden items'
      : 'No entries',
);

const baseColumn = computed(() => ({
  path: props.directoryKey,
  title: props.directoryKey,
  entries: props.entries,
  rawEntryCount: props.rawEntryCount,
  selectedIndex: props.selectedIndex,
  loading: props.loading,
  error: '',
  base: true,
}));

const columns = computed(() => [baseColumn.value, ...columnTrail.value]);
const parentDirectory = computed(() => parentPathForDirectory(props.directoryKey));

function activeScroller() {
  if (props.viewMode === 'grid') {
    return gridScroller.value;
  }

  if (props.viewMode === 'columns') {
    return columnScroller.value;
  }

  return listScroller.value;
}

function resetScroll() {
  nextTick(() => {
    const scroller = activeScroller();

    if (scroller) {
      scroller.scrollTop = 0;
    }
  });
}

function scrollSelectedIntoView() {
  nextTick(() => {
    if (props.selectedIndex < 0) {
      return;
    }

    const scroller = activeScroller();
    const item = scroller?.querySelector(`[data-file-index="${props.selectedIndex}"]`);

    item?.scrollIntoView({ block: 'nearest' });
  });
}

function normalizeSortKey(sortKey) {
  return SORT_KEYS.includes(sortKey) ? sortKey : 'name';
}

function normalizeSortDirection(sortDirection) {
  return sortDirection === 'desc' ? 'desc' : 'asc';
}

function kindRank(entry) {
  if (entry?.kind === 'directory') return 0;
  if (entry?.kind === 'file') return 1;
  if (entry?.kind === 'symlink') return 2;
  return 3;
}

function extensionForName(name) {
  const value = String(name || '');
  const dotIndex = value.lastIndexOf('.');

  if (dotIndex <= 0 || dotIndex === value.length - 1) {
    return '';
  }

  return value.slice(dotIndex + 1).toLowerCase();
}

function compareNames(a, b) {
  return NAME_COLLATOR.compare(a.name, b.name) || a.name.localeCompare(b.name);
}

function compareOptionalNumber(a, b, fallback = 0) {
  return (a ?? fallback) - (b ?? fallback);
}

function parentPathForDirectory(path) {
  const value = String(path || '').trim();

  if (isArchivePath(value)) {
    return archiveParentPath(value);
  }

  if (!value || value === '/' || value === '~') {
    return '';
  }

  if (value.startsWith('remote://')) {
    const trimmed = value.endsWith('/') ? value.slice(0, -1) : value;
    const rest = trimmed.slice('remote://'.length);
    const slashIndex = rest.indexOf('/');
    const volumeId = slashIndex >= 0 ? rest.slice(0, slashIndex) : rest;
    const objectPath = slashIndex >= 0 ? rest.slice(slashIndex + 1).replace(/\/+$/, '') : '';

    if (!volumeId || !objectPath) {
      return '';
    }

    const parentIndex = objectPath.lastIndexOf('/');
    return parentIndex < 0
      ? `remote://${volumeId}/`
      : `remote://${volumeId}/${objectPath.slice(0, parentIndex)}`;
  }

  if (value.startsWith('~/')) {
    const rest = value.slice(2).replace(/\/+$/, '');
    const index = rest.lastIndexOf('/');

    return index < 0 ? '~' : `~/${rest.slice(0, index)}`;
  }

  const trimmed = value.replace(/\/+$/, '');
  const index = trimmed.lastIndexOf('/');

  if (index < 0) {
    return '~';
  }

  return index === 0 ? '/' : trimmed.slice(0, index);
}

function openParentDirectory(event) {
  if (!parentDirectory.value) {
    return;
  }

  event?.stopPropagation?.();
  emit('open-parent', parentDirectory.value);
}

function sortEntries(entries) {
  const sortKey = normalizeSortKey(props.sortKey);
  const multiplier = normalizeSortDirection(props.sortDirection) === 'desc' ? -1 : 1;

  if (sortKey === 'none') {
    return [...entries];
  }

  return [...entries].sort((a, b) => {
    const kindOrder = kindRank(a) - kindRank(b);

    if (kindOrder !== 0) {
      return kindOrder;
    }

    let sortOrder = 0;

    if (sortKey === 'extension') {
      sortOrder = NAME_COLLATOR.compare(extensionForName(a.name), extensionForName(b.name));
    } else if (sortKey === 'size') {
      sortOrder = compareOptionalNumber(a.size, b.size, -1);
    } else if (sortKey === 'modifiedAt') {
      sortOrder = compareOptionalNumber(a.modifiedAt, b.modifiedAt, 0);
    } else {
      sortOrder = compareNames(a, b);
    }

    return sortOrder !== 0 ? sortOrder * multiplier : compareNames(a, b);
  });
}

function visibleEntriesForColumn(column) {
  if (column.base) {
    return column.entries;
  }

  const visibleEntries = props.showHiddenFiles
    ? column.entries
    : column.entries.filter((entry) => !entry.isHidden);
  const entries = activeSearchQuery.value
    ? visibleEntries.filter((entry) => String(entry.name || '').toLowerCase().includes(activeSearchQuery.value))
    : visibleEntries;

  return sortEntries(entries);
}

function emptyMessageForColumn(column) {
  const rawCount = column.rawEntryCount ?? column.entries?.length ?? 0;
  const visibleCount = visibleEntriesForColumn(column).length;

  if (isSearchFiltering.value && rawCount > 0 && visibleCount === 0) {
    return 'No items match your search.';
  }

  if (!isSearchFiltering.value && !props.showHiddenFiles && rawCount > 0 && visibleCount === 0) {
    return 'Only hidden items';
  }

  return 'No entries';
}

function selectedEntriesForColumn(column, columnIndex) {
  const visibleEntries = visibleEntriesForColumn(column);

  if (columnIndex === 0) {
    return visibleEntries.filter((entry, index) => props.isEntrySelected(index));
  }

  const selectedPaths = new Set(column.selectedPaths || []);

  if (selectedPaths.size > 0) {
    return visibleEntries.filter((entry) => selectedPaths.has(entry.path));
  }

  const focusedEntry = visibleEntries[column.selectedIndex];
  return focusedEntry ? [focusedEntry] : [];
}

function focusedEntryForColumn(column, columnIndex) {
  const visibleEntries = visibleEntriesForColumn(column);

  if (columnIndex === 0) {
    return visibleEntries[props.selectedIndex] || selectedEntriesForColumn(column, columnIndex)[0] || null;
  }

  return visibleEntries[column.selectedIndex] || selectedEntriesForColumn(column, columnIndex)[0] || null;
}

function resetColumnTrail() {
  columnLoadVersion += 1;
  columnTrail.value = [];
}

function activeColumnDirectory() {
  if (props.viewMode !== 'columns') {
    return props.directoryKey;
  }

  return columnTrail.value.at(-1)?.path || props.directoryKey;
}

function emitActiveDirectory() {
  emit('active-directory', activeColumnDirectory());
}

function activeColumnState() {
  const activeColumnIndex = Math.max(0, columns.value.length - 1);
  const column = columns.value[activeColumnIndex];

  if (!column) {
    return null;
  }

  const visibleEntries = visibleEntriesForColumn(column);
  const selectedEntries = selectedEntriesForColumn(column, activeColumnIndex);

  return {
    path: column.path,
    title: column.title,
    loading: Boolean(column.loading),
    error: column.error || '',
    entries: visibleEntries,
    rawEntryCount: column.rawEntryCount ?? column.entries?.length ?? visibleEntries.length,
    searchQuery: props.searchQuery.trim(),
    showHiddenFiles: props.showHiddenFiles,
    selectedEntries,
    focusedEntry: focusedEntryForColumn(column, activeColumnIndex),
  };
}

function emitColumnSummary() {
  if (props.viewMode !== 'columns') {
    emit('column-summary', null);
    return;
  }

  emit('column-summary', activeColumnState());
}

function columnSelectionClass(column, columnIndex, index) {
  if (columnIndex === 0) {
    return props.isEntrySelected(index);
  }

  const selectedPaths = new Set(column.selectedPaths || []);

  if (selectedPaths.size > 0) {
    const entry = visibleEntriesForColumn(column)[index];
    return entry ? selectedPaths.has(entry.path) : false;
  }

  return column.selectedIndex === index;
}

function updateColumnSelection(columnIndex, selectedIndex, selectedPaths = [], selectionAnchorIndex = selectedIndex) {
  if (columnIndex === 0) {
    return [];
  }

  const nextTrail = columnTrail.value.slice(0, columnIndex);
  const trailIndex = columnIndex - 1;

  if (nextTrail[trailIndex]) {
    nextTrail[trailIndex] = {
      ...nextTrail[trailIndex],
      selectedIndex,
      selectedPaths,
      selectionAnchorIndex,
    };
  }

  return nextTrail;
}

function updateChildColumnSelection(entry, index, columnIndex, event = null) {
  const column = columns.value[columnIndex];

  if (!column || columnIndex === 0) {
    return;
  }

  const visibleEntries = visibleEntriesForColumn(column);

  if (event?.shiftKey) {
    const anchorIndex = Number.isInteger(column.selectionAnchorIndex) && column.selectionAnchorIndex >= 0
      ? column.selectionAnchorIndex
      : Number.isInteger(column.selectedIndex) && column.selectedIndex >= 0
        ? column.selectedIndex
        : index;
    const start = Math.min(anchorIndex, index);
    const end = Math.max(anchorIndex, index);
    const selectedPaths = visibleEntries.slice(start, end + 1).map((candidate) => candidate.path);

    columnTrail.value = updateColumnSelection(columnIndex, index, selectedPaths, anchorIndex);
    emit('preview-entry', entry);
    return;
  }

  if (event?.metaKey || event?.ctrlKey) {
    const selectedPaths = new Set(column.selectedPaths || []);
    const implicitEntry = visibleEntries[column.selectedIndex];
    const isImplicitSelection = selectedPaths.size === 0 && column.selectedIndex === index;
    const isSelected = selectedPaths.has(entry.path) || isImplicitSelection;

    if (selectedPaths.size === 0 && implicitEntry && !isImplicitSelection) {
      selectedPaths.add(implicitEntry.path);
    }

    if (isSelected) {
      selectedPaths.delete(entry.path);
    } else {
      selectedPaths.add(entry.path);
    }

    const nextPaths = [...selectedPaths];
    columnTrail.value = updateColumnSelection(
      columnIndex,
      nextPaths.length > 0 ? index : -1,
      nextPaths,
      index,
    );
    emit('preview-entry', nextPaths.length > 0 ? entry : null);
    return;
  }

  columnTrail.value = updateColumnSelection(columnIndex, index, [], index);
  emit('preview-entry', entry);
}

function scrollColumnsToEnd() {
  nextTick(() => {
    if (columnScroller.value) {
      columnScroller.value.scrollLeft = columnScroller.value.scrollWidth;
    }
  });
}

async function loadChildColumn(entry, columnIndex, selectedIndex = -1) {
  const nextTrail = updateColumnSelection(columnIndex, selectedIndex);

  if (!isBrowsableEntry(entry)) {
    columnTrail.value = nextTrail;
    return;
  }

  const childPath = entry.kind === 'directory' ? entry.path : archiveRootPath(entry.path);
  const childPosition = columnIndex;
  const loadVersion = columnLoadVersion + 1;
  columnLoadVersion = loadVersion;
  columnTrail.value = [
    ...nextTrail,
    {
      path: childPath,
      title: entry.name,
      entries: [],
      rawEntryCount: 0,
      selectedIndex: -1,
      selectionAnchorIndex: -1,
      selectedPaths: [],
      loading: true,
      error: '',
      base: false,
    },
  ];
  scrollColumnsToEnd();

  try {
    const entries = await listDirectory(childPath);

    if (columnLoadVersion !== loadVersion || columnTrail.value[childPosition]?.path !== childPath) {
      return;
    }

    columnTrail.value = [
      ...columnTrail.value.slice(0, childPosition),
      {
        ...columnTrail.value[childPosition],
        entries,
        rawEntryCount: entries.length,
        loading: false,
        error: '',
      },
    ];
    scrollColumnsToEnd();
  } catch (error) {
    if (columnLoadVersion !== loadVersion || columnTrail.value[childPosition]?.path !== childPath) {
      return;
    }

    columnTrail.value = [
      ...columnTrail.value.slice(0, childPosition),
      {
        ...columnTrail.value[childPosition],
        loading: false,
        error: error?.message || 'Unable to load folder.',
      },
    ];
  }
}

function handleColumnSelect(entry, index, columnIndex, event = null) {
  const isModifiedSelection = Boolean(event?.shiftKey || event?.metaKey || event?.ctrlKey);

  if (columnIndex === 0) {
    emit('select', { index, event });

    if (isModifiedSelection) {
      resetColumnTrail();
      return;
    }
  } else {
    updateChildColumnSelection(entry, index, columnIndex, event);

    if (isModifiedSelection || !isBrowsableEntry(entry)) {
      return;
    }
  }

  loadChildColumn(entry, columnIndex, index);
}

function handleColumnOpen(entry, index, columnIndex) {
  if (columnIndex === 0) {
    emit('open', index);
    return;
  }

  emit('open-path', entry);
}

function handleColumnContext(entry, index, columnIndex, event) {
  let operationEntries = [entry];

  if (columnIndex !== 0) {
    const column = columns.value[columnIndex];
    const isSelected = columnSelectionClass(column, columnIndex, index);

    if (isSelected) {
      operationEntries = selectedEntriesForColumn(column, columnIndex);
    } else {
      columnTrail.value = updateColumnSelection(columnIndex, index, [], index);
    }

    emit('preview-entry', entry);
  }

  emit('context', {
    index: columnIndex === 0 ? index : null,
    entry,
    operationEntries,
    x: event.clientX,
    y: event.clientY,
  });
}

function cleanPath(path) {
  const value = String(path || '');

  if (isArchivePath(value)) {
    return value.endsWith('!/') ? value : value.replace(/\/+$/, '');
  }

  return value.replace(/\/+$/, '') || '/';
}

function isSameOrChildPath(path, parentPath) {
  const child = cleanPath(path);
  const parent = cleanPath(parentPath);

  return child === parent || (parent !== '/' && child.startsWith(`${parent}/`));
}

function canDropOnEntry(entry) {
  if (!props.dragging || entry?.kind !== 'directory' || isArchivePath(entry?.path)) {
    return false;
  }

  const targetPath = cleanPath(entry.path);

  return !props.draggedPaths.some((path) => isSameOrChildPath(targetPath, path));
}

function canDropOnDirectory(directoryPath = activeColumnDirectory()) {
  if (!props.dragging || isArchivePath(directoryPath)) {
    return false;
  }

  const targetPath = cleanPath(directoryPath);
  return !props.draggedPaths.some((path) => isSameOrChildPath(targetPath, path));
}

function isFileRowTarget(event) {
  return Boolean(event.target?.closest?.('.file-list-item, .file-grid-item, .file-column-row, .file-parent-row, .file-parent-card'));
}

function clearDropTarget() {
  entryDropPath.value = '';
  currentDirectoryDropActive.value = false;
  currentDirectoryDropPath.value = '';
}

function handleEntryDragStart(entry, index, columnIndex, event) {
  emitEntryDragStart(entry, index, columnIndex, event, 'drag-start');
}

function handleEntryPointerDown(entry, index, columnIndex, event) {
  let operationEntries = [];

  if (columnIndex !== 0) {
    const column = columns.value[columnIndex];

    if (columnSelectionClass(column, columnIndex, index)) {
      operationEntries = selectedEntriesForColumn(column, columnIndex);
    }
  }

  emit('pointer-drag-start', {
    entry,
    index: columnIndex === 0 ? index : null,
    columnIndex,
    operationEntries,
    event,
  });
}

function emitEntryDragStart(entry, index, columnIndex, event, eventName) {
  let operationEntries = [];

  if (columnIndex !== 0) {
    const column = columns.value[columnIndex];
    const isSelected = columnSelectionClass(column, columnIndex, index);

    if (isSelected) {
      operationEntries = selectedEntriesForColumn(column, columnIndex);
    } else {
      columnTrail.value = updateColumnSelection(columnIndex, index, [], index);
      operationEntries = [entry];
      emit('preview-entry', entry);
    }
  }

  emit(eventName, {
    entry,
    index: columnIndex === 0 ? index : null,
    columnIndex,
    operationEntries,
    event,
  });
}

function handleEntryDragOver(entry, event, directoryPath = activeColumnDirectory()) {
  const canDropIntoEntry = canDropOnEntry(entry);
  const canDropIntoCurrent = !canDropIntoEntry && canDropOnDirectory(directoryPath);

  if (!canDropIntoEntry && !canDropIntoCurrent) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = dropEffectFromEvent(event);
  }
  entryDropPath.value = canDropIntoEntry ? entry.path : '';
  currentDirectoryDropActive.value = canDropIntoCurrent;
  currentDirectoryDropPath.value = canDropIntoCurrent ? directoryPath : '';
}

function handleEntryDragLeave(entry, event) {
  if (entryDropPath.value !== entry.path || event.currentTarget.contains(event.relatedTarget)) {
    return;
  }

  entryDropPath.value = '';
}

function handleEntryDrop(entry, event, directoryPath = activeColumnDirectory()) {
  const canDropIntoEntry = canDropOnEntry(entry);
  const canDropIntoCurrent = !canDropIntoEntry && canDropOnDirectory(directoryPath);

  if (!canDropIntoEntry && !canDropIntoCurrent) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  clearDropTarget();

  if (canDropIntoEntry) {
    emit('drop-entry', { entry, event });
  } else {
    emit('drop-current', { event, targetDirectory: directoryPath });
  }
}

function handleCurrentDirectoryDragOver(event, directoryPath = activeColumnDirectory()) {
  if (!canDropOnDirectory(directoryPath) || isFileRowTarget(event)) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = dropEffectFromEvent(event);
  }
  entryDropPath.value = '';
  currentDirectoryDropActive.value = true;
  currentDirectoryDropPath.value = directoryPath;
}

function handleCurrentDirectoryDragLeave(event) {
  if (event.currentTarget.contains(event.relatedTarget)) {
    return;
  }

  currentDirectoryDropActive.value = false;
  currentDirectoryDropPath.value = '';
}

function handleCurrentDirectoryDrop(event, directoryPath = activeColumnDirectory()) {
  if (!canDropOnDirectory(directoryPath) || isFileRowTarget(event)) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  clearDropTarget();
  emit('drop-current', { event, targetDirectory: directoryPath });
}

function isCurrentDirectoryDropTarget(directoryPath) {
  return currentDirectoryDropActive.value
    && cleanPath(currentDirectoryDropPath.value) === cleanPath(directoryPath);
}

function handleDragEnd(event) {
  clearDropTarget();
  emit('drag-end', event);
}

function handleBackgroundClick(event) {
  if (event.target?.closest?.('button, .file-list-item, .file-grid-item, .file-column-row')) {
    return;
  }

  if (props.viewMode === 'columns' && columnTrail.value.length > 0) {
    const lastIndex = columnTrail.value.length;
    columnTrail.value = updateColumnSelection(lastIndex, -1, [], -1);
    emit('preview-entry', null);
  }

  emit('background-click', { event });
}

function syncBaseColumnSelection() {
  if (props.viewMode !== 'columns' || props.loading) {
    return;
  }

  const entry = props.entries[props.selectedIndex];
  const selectedEntries = props.entries.filter((candidate, index) => props.isEntrySelected(index));

  if (selectedEntries.length !== 1 || !entry || !isBrowsableEntry(entry)) {
    resetColumnTrail();
    return;
  }

  const childPath = entry.kind === 'directory' ? entry.path : archiveRootPath(entry.path);

  if (columnTrail.value[0]?.path === childPath) {
    return;
  }

  loadChildColumn(entry, 0, props.selectedIndex);
}

async function refreshColumnDirectory(path) {
  const requestedPath = cleanPath(path);

  if (props.viewMode !== 'columns' || requestedPath === cleanPath(props.directoryKey)) {
    return;
  }

  const columnIndex = columnTrail.value.findIndex((column) => cleanPath(column.path) === requestedPath);

  if (columnIndex < 0) {
    return;
  }

  const columnPath = columnTrail.value[columnIndex].path;
  columnTrail.value = [
    ...columnTrail.value.slice(0, columnIndex),
    {
      ...columnTrail.value[columnIndex],
      loading: true,
      error: '',
    },
    ...columnTrail.value.slice(columnIndex + 1),
  ];

  try {
    const entries = await listDirectory(columnPath);
    const currentIndex = columnTrail.value.findIndex((column) => cleanPath(column.path) === requestedPath);

    if (currentIndex < 0) {
      return;
    }

    const selectedIndex = Math.min(
      columnTrail.value[currentIndex].selectedIndex,
      entries.length - 1,
    );
    const refreshedPaths = new Set(entries.map((entry) => entry.path));

    columnTrail.value = [
      ...columnTrail.value.slice(0, currentIndex),
      {
        ...columnTrail.value[currentIndex],
        entries,
        rawEntryCount: entries.length,
        selectedIndex,
        selectedPaths: (columnTrail.value[currentIndex].selectedPaths || [])
          .filter((path) => refreshedPaths.has(path)),
        loading: false,
        error: '',
      },
      ...columnTrail.value.slice(currentIndex + 1),
    ];
  } catch (error) {
    const currentIndex = columnTrail.value.findIndex((column) => cleanPath(column.path) === requestedPath);

    if (currentIndex < 0) {
      return;
    }

    columnTrail.value = [
      ...columnTrail.value.slice(0, currentIndex),
      {
        ...columnTrail.value[currentIndex],
        loading: false,
        error: error?.message || 'Unable to refresh folder.',
      },
      ...columnTrail.value.slice(currentIndex + 1),
    ];
  }
}

watch(() => props.directoryKey, () => {
  resetColumnTrail();
  resetScroll();
});
watch(() => props.viewMode, resetScroll);
watch(
  () => [props.viewMode, props.directoryKey, columnTrail.value.map((column) => column.path).join('\u0000')],
  emitActiveDirectory,
  { immediate: true },
);
watch(
  () => [
    props.viewMode,
    props.directoryKey,
    props.entries,
    props.selectedIndex,
    props.loading,
    props.rawEntryCount,
    props.searchQuery,
    props.showHiddenFiles,
    props.sortKey,
    props.sortDirection,
    columnTrail.value,
  ],
  emitColumnSummary,
  { immediate: true, deep: true, flush: 'post' },
);
watch(
  () => props.columnRefreshRequest?.id,
  () => {
    if (props.columnRefreshRequest?.path) {
      refreshColumnDirectory(props.columnRefreshRequest.path);
    }
  },
);
watch(() => props.refreshKey, () => {
  if (props.viewMode !== 'columns') {
    resetColumnTrail();
  }

  syncBaseColumnSelection();
});
watch(() => [props.sortKey, props.sortDirection, props.showHiddenFiles], () => {
  resetColumnTrail();
  syncBaseColumnSelection();
  resetScroll();
});
watch(() => props.selectedIndex, () => {
  scrollSelectedIntoView();
  syncBaseColumnSelection();
});
watch(
  () => [props.viewMode, props.directoryKey, props.entries, props.loading],
  syncBaseColumnSelection,
  { flush: 'post' },
);
</script>

<template>
  <div
    class="file-list-root"
    :class="{ 'file-list-root--drop-target': currentDirectoryDropActive && viewMode !== 'columns' }"
    :data-drop-directory-path="activeColumnDirectory()"
    :aria-busy="loading"
    @dragover="handleCurrentDirectoryDragOver"
    @dragleave="handleCurrentDirectoryDragLeave"
    @drop="handleCurrentDirectoryDrop"
    @click="handleBackgroundClick"
  >
    <template v-if="loading">
      <div class="file-loading-state" role="status" aria-live="polite">
        <span class="visually-hidden">Loading directory contents</span>

        <div
          v-if="viewMode === 'grid'"
          class="file-grid-scroller file-loading-scroller"
          aria-label="Loading directory grid"
        >
          <div class="file-grid-window">
            <div v-for="index in 10" :key="`grid-loader-${index}`" class="file-grid-item">
              <div class="file-card file-card--loading">
                <span class="file-card-frame loading-card-frame" aria-hidden="true"></span>
                <span class="loading-line loading-line--label" aria-hidden="true"></span>
              </div>
            </div>
          </div>
        </div>

        <div
          v-else-if="viewMode === 'columns'"
          class="file-column-scroller file-loading-scroller"
          aria-label="Loading directory columns"
        >
          <div class="file-column-track">
            <section class="file-column">
              <div v-for="index in 18" :key="`column-loader-${index}`" class="loading-column-row">
                <span class="loading-glyph" aria-hidden="true"></span>
                <span class="loading-line" aria-hidden="true"></span>
              </div>
            </section>
          </div>
        </div>

        <div v-else class="file-list-frame" role="table" aria-label="Loading directory listing">
          <div class="file-list-header file-list-header--loading" role="row">
            <span role="columnheader">Name</span>
            <span role="columnheader"></span>
            <span role="columnheader">Size</span>
            <span role="columnheader">Modified</span>
          </div>

          <div class="file-list file-list--loading">
            <div v-for="index in 18" :key="`list-loader-${index}`" class="loading-row" role="row">
              <span class="loading-name-cell">
                <span class="loading-glyph" aria-hidden="true"></span>
                <span class="loading-line" aria-hidden="true"></span>
              </span>
              <span class="loading-dot" aria-hidden="true"></span>
              <span class="loading-line loading-line--size" aria-hidden="true"></span>
              <span class="loading-line loading-line--date" aria-hidden="true"></span>
            </div>
          </div>
        </div>
      </div>
    </template>
    <div
      v-else-if="entries.length === 0 && !parentDirectory && viewMode !== 'columns'"
      class="file-list-empty"
    >
      {{ emptyDirectoryMessage }}
    </div>

    <div
      v-else-if="viewMode === 'grid'"
      ref="gridScroller"
      class="file-grid-scroller"
      role="list"
      aria-label="Directory grid"
      :data-drop-directory-path="directoryKey"
    >
      <div class="file-grid-window">
        <div v-if="parentDirectory" class="file-grid-item file-parent-grid-item">
          <button
            type="button"
            class="file-parent-card"
            :title="`Go to ${parentDirectory}`"
            @click="openParentDirectory"
            @keydown.stop
          >
            <span class="file-parent-card-frame" aria-hidden="true">
              <AppIcon name="folder" :size="58" :stroke-width="1.55" />
              <AppIcon class="file-parent-card-arrow" name="chevron-down" :size="18" :stroke-width="2.2" />
            </span>
            <span class="file-parent-card-name">Parent Folder</span>
          </button>
        </div>

        <div v-if="entries.length === 0 && parentDirectory" class="file-grid-empty-message">
          {{ emptyDirectoryMessage }}
        </div>

        <div
          v-for="(entry, index) in entries"
          :key="entry.path"
          v-memo="[entry.path, entry.name, entry.size, entry.modifiedAt, isEntrySelected(index), entryDropPath === entry.path, dateFormat]"
          class="file-grid-item"
          :class="{ 'file-drop-target': entryDropPath === entry.path }"
          :data-file-index="index"
          :data-drop-entry-path="entry.path"
          :data-drop-entry-kind="entry.kind"
          data-file-drag-source="true"
          @pointerdown="handleEntryPointerDown(entry, index, 0, $event)"
          @dragstart.stop="handleEntryDragStart(entry, index, 0, $event)"
          @dragend="handleDragEnd"
          @dragover="handleEntryDragOver(entry, $event, directoryKey)"
          @dragleave="handleEntryDragLeave(entry, $event)"
          @drop="handleEntryDrop(entry, $event, directoryKey)"
          @contextmenu.prevent="$emit('context', { index, entry, x: $event.clientX, y: $event.clientY })"
        >
          <FileRow
            :entry="entry"
            :selected="isEntrySelected(index)"
            :date-format="dateFormat"
            variant="grid"
            @click="$emit('select', { index, event: $event })"
            @open="$emit('open', index)"
          />
        </div>
      </div>
    </div>

    <div
      v-else-if="viewMode === 'columns'"
      ref="columnScroller"
      class="file-column-scroller"
      role="listbox"
      aria-multiselectable="true"
      aria-label="Directory columns"
      :data-drop-directory-path="activeColumnDirectory()"
    >
      <div class="file-column-track">
        <section
          v-for="(column, columnIndex) in columns"
          :key="`${column.path}-${columnIndex}`"
          class="file-column"
          :class="{ 'file-column--drop-target': isCurrentDirectoryDropTarget(column.path) }"
          :data-drop-directory-path="column.path"
          :aria-label="columnIndex === 0 ? 'Current folder' : column.title"
          @dragover="handleCurrentDirectoryDragOver($event, column.path)"
          @dragleave="handleCurrentDirectoryDragLeave"
          @drop="handleCurrentDirectoryDrop($event, column.path)"
        >
          <div v-if="column.loading" class="file-column-loading" aria-live="polite">
            <div v-for="index in 12" :key="`column-${columnIndex}-loading-${index}`" class="loading-column-row">
              <span class="loading-glyph" aria-hidden="true"></span>
              <span class="loading-line" aria-hidden="true"></span>
            </div>
          </div>

          <p v-else-if="column.error" class="file-column-message">{{ column.error }}</p>
          <p v-else-if="visibleEntriesForColumn(column).length === 0" class="file-column-message">
            {{ emptyMessageForColumn(column) }}
          </p>

          <template v-else>
            <button
              v-for="(entry, index) in visibleEntriesForColumn(column)"
              :key="entry.path"
              type="button"
              class="file-column-row"
              :class="{
                'file-column-row--selected': columnSelectionClass(column, columnIndex, index),
                'file-column-row--directory': entry.kind === 'directory',
                'file-column-row--archive': isArchiveEntry(entry),
                'file-drop-target': entryDropPath === entry.path,
              }"
              :data-file-index="columnIndex === 0 ? index : null"
              :data-drop-entry-path="entry.path"
              :data-drop-entry-kind="entry.kind"
              data-file-drag-source="true"
              :aria-selected="columnSelectionClass(column, columnIndex, index)"
              :title="entry.name"
              @pointerdown="handleEntryPointerDown(entry, index, columnIndex, $event)"
              @dragstart.stop="handleEntryDragStart(entry, index, columnIndex, $event)"
              @dragend="handleDragEnd"
              @dragover="handleEntryDragOver(entry, $event, column.path)"
              @dragleave="handleEntryDragLeave(entry, $event)"
              @drop="handleEntryDrop(entry, $event, column.path)"
              @click="handleColumnSelect(entry, index, columnIndex, $event)"
              @dblclick="handleColumnOpen(entry, index, columnIndex)"
              @contextmenu.prevent="handleColumnContext(entry, index, columnIndex, $event)"
            >
              <span class="file-column-glyph" :class="`file-column-glyph--${entry.kind}`">
                <AppIcon
                  :name="entry.kind === 'directory' ? 'folder' : isArchiveEntry(entry) ? 'archive' : 'file'"
                  :size="17"
                  :stroke-width="1.8"
                />
              </span>
              <span class="file-column-name">{{ entry.name }}</span>
              <AppIcon
                v-if="isBrowsableEntry(entry)"
                class="file-column-chevron"
                name="chevron-right"
                :size="14"
                :stroke-width="2.1"
              />
            </button>
          </template>
        </section>
      </div>
    </div>

    <div v-else class="file-list-frame" role="table" aria-label="Directory listing">
      <div class="file-list-header" role="row">
        <button
          type="button"
          class="file-list-heading"
          :class="{ 'file-list-heading--active': sortKey === 'name' }"
          role="columnheader"
          :aria-sort="sortKey === 'name' ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'"
          @click="$emit('sort', 'name')"
          @keydown.stop
        >
          <span>Name</span>
          <AppIcon
            v-if="sortKey === 'name'"
            name="chevron-down"
            :size="12"
            :stroke-width="2.1"
            :class="{ 'sort-icon--asc': sortDirection === 'asc' }"
          />
        </button>
        <span role="columnheader"></span>
        <button
          type="button"
          class="file-list-heading file-list-heading--end"
          :class="{ 'file-list-heading--active': sortKey === 'size' }"
          role="columnheader"
          :aria-sort="sortKey === 'size' ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'"
          @click="$emit('sort', 'size')"
          @keydown.stop
        >
          <span>Size</span>
          <AppIcon
            v-if="sortKey === 'size'"
            name="chevron-down"
            :size="12"
            :stroke-width="2.1"
            :class="{ 'sort-icon--asc': sortDirection === 'asc' }"
          />
        </button>
        <button
          type="button"
          class="file-list-heading file-list-heading--end"
          :class="{ 'file-list-heading--active': sortKey === 'modifiedAt' }"
          role="columnheader"
          :aria-sort="sortKey === 'modifiedAt' ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'"
          @click="$emit('sort', 'modifiedAt')"
          @keydown.stop
        >
          <span>Modified</span>
          <AppIcon
            v-if="sortKey === 'modifiedAt'"
            name="chevron-down"
            :size="12"
            :stroke-width="2.1"
            :class="{ 'sort-icon--asc': sortDirection === 'asc' }"
          />
        </button>
      </div>

      <div ref="listScroller" class="file-list" :data-drop-directory-path="directoryKey">
        <div v-if="parentDirectory" class="file-list-item file-parent-list-item">
          <button
            type="button"
            class="file-parent-row"
            :title="`Go to ${parentDirectory}`"
            @click="openParentDirectory"
            @keydown.stop
          >
            <span class="file-parent-name">
              <span class="file-parent-glyph" aria-hidden="true">
                <AppIcon name="folder" :size="18" :stroke-width="1.8" />
              </span>
              <span>..</span>
            </span>
            <span></span>
            <span class="file-parent-muted">Parent</span>
            <span class="file-parent-muted file-parent-path">{{ parentDirectory }}</span>
          </button>
        </div>

        <div v-if="entries.length === 0 && parentDirectory" class="file-list-empty file-list-empty--inline">
          {{ emptyDirectoryMessage }}
        </div>

        <div
          v-for="(entry, index) in entries"
          :key="entry.path"
          v-memo="[entry.path, entry.name, entry.size, entry.modifiedAt, isEntrySelected(index), entryDropPath === entry.path, dateFormat]"
          class="file-list-item"
          :class="{ 'file-drop-target': entryDropPath === entry.path }"
          :data-file-index="index"
          :data-drop-entry-path="entry.path"
          :data-drop-entry-kind="entry.kind"
          data-file-drag-source="true"
          @pointerdown="handleEntryPointerDown(entry, index, 0, $event)"
          @dragstart.stop="handleEntryDragStart(entry, index, 0, $event)"
          @dragend="handleDragEnd"
          @dragover="handleEntryDragOver(entry, $event, directoryKey)"
          @dragleave="handleEntryDragLeave(entry, $event)"
          @drop="handleEntryDrop(entry, $event, directoryKey)"
          @contextmenu.prevent="$emit('context', { index, entry, x: $event.clientX, y: $event.clientY })"
        >
          <FileRow
            :entry="entry"
            :selected="isEntrySelected(index)"
            :date-format="dateFormat"
            variant="list"
            @click="$emit('select', { index, event: $event })"
            @open="$emit('open', index)"
          />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.file-list-root {
  height: 100%;
  min-height: 0;
  outline: 1px solid transparent;
  outline-offset: -1px;
  transition: outline-color 100ms ease, background 100ms ease;
}

.file-list-root--drop-target {
  background: rgb(var(--accent-rgb) / 0.055);
  outline-color: rgb(var(--accent-rgb) / 0.42);
}

.file-loading-state {
  height: 100%;
  min-height: 0;
}

.file-list-frame {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  contain: layout paint style;
}

.file-list-header {
  flex: 0 0 auto;
  display: grid;
  grid-template-columns: minmax(180px, 1fr) 46px 88px 126px;
  gap: 12px;
  min-height: 35px;
  align-items: center;
  padding: 0 20px 0 34px;
  border-bottom: 1px solid var(--hairline);
  background: color-mix(in srgb, var(--text) 4%, transparent);
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 680;
  letter-spacing: 0;
  text-transform: none;
}

.file-list-header--loading {
  pointer-events: none;
}

.file-list-header--loading span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-list-heading {
  display: inline-flex;
  min-width: 0;
  align-items: center;
  gap: 4px;
  padding: 0;
  background: transparent;
  color: inherit;
  font: inherit;
  letter-spacing: inherit;
  text-align: left;
  text-transform: inherit;
}

.file-list-heading span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-list-heading--end {
  justify-content: flex-end;
  text-align: right;
}

.file-list-heading--active {
  color: var(--text-muted);
}

.sort-icon--asc {
  transform: rotate(180deg);
}

.file-list {
  height: 100%;
  min-height: 0;
  flex: 1 1 auto;
  overflow: auto;
  padding: 6px 0 18px;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
}

.file-list--loading {
  display: flex;
  flex-direction: column;
}

.file-list-item {
  height: 29px;
  contain: layout paint style;
}

.file-parent-list-item {
  contain: layout paint style;
}

.file-parent-row {
  display: grid;
  width: 100%;
  height: 29px;
  grid-template-columns: minmax(180px, 1fr) 46px 88px 126px;
  align-items: center;
  gap: 12px;
  padding: 2px 20px 2px 34px;
  background: transparent;
  color: var(--text);
  text-align: left;
  transition: background 80ms ease;
}

.file-parent-row:hover {
  background: var(--btn-hover);
}

.file-parent-name {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 7px;
  font-size: 14px;
  font-weight: 650;
}

.file-parent-glyph {
  display: grid;
  width: 18px;
  height: 18px;
  flex: 0 0 auto;
  place-items: center;
  color: var(--folder-icon);
}

.file-parent-muted {
  overflow: hidden;
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 560;
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-parent-path {
  color: var(--text-faint);
}

.file-list-item.file-drop-target :deep(.file-row) {
  background: rgb(var(--accent-rgb) / 0.26);
  box-shadow:
    inset 0 0 0 1px rgb(var(--accent-rgb) / 0.72),
    inset 0 1px 0 rgb(255 255 255 / 0.14);
}

.file-list-empty {
  padding: 24px 20px;
  color: var(--text-muted);
  font-size: 14px;
}

.file-list-empty--inline {
  padding-left: 34px;
}

.loading-row {
  display: grid;
  grid-template-columns: minmax(180px, 1fr) 46px 88px 126px;
  align-items: center;
  gap: 12px;
  height: 29px;
  padding: 2px 20px 2px 34px;
  contain: layout paint style;
}

.loading-name-cell {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 8px;
}

.loading-glyph,
.loading-dot,
.loading-line,
.loading-card-frame {
  position: relative;
  overflow: hidden;
  background:
    linear-gradient(90deg, transparent, color-mix(in srgb, var(--text) 6%, transparent), transparent),
    color-mix(in srgb, var(--text) 5%, transparent);
  background-size: 220% 100%, 100% 100%;
  animation: loading-sheen 1350ms cubic-bezier(0.35, 0, 0.2, 1) infinite;
}

.loading-glyph {
  width: 18px;
  height: 15px;
  flex: 0 0 auto;
  border-radius: 3px;
}

.loading-dot {
  width: 11px;
  height: 11px;
  justify-self: center;
  border-radius: 50%;
  opacity: 0.75;
}

.loading-line {
  display: block;
  width: min(62%, 260px);
  height: 10px;
  min-width: 44px;
  border-radius: 999px;
}

.loading-line--size {
  width: 58px;
  justify-self: end;
}

.loading-line--date {
  width: 92px;
  justify-self: end;
}

.loading-line--label {
  width: 112px;
  height: 13px;
}

.file-grid-scroller {
  height: 100%;
  min-height: 0;
  overflow: auto;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
  contain: layout paint style;
}

.file-loading-scroller {
  pointer-events: none;
}

.file-grid-item {
  min-width: 0;
  contain: layout paint style;
}

.file-parent-grid-item {
  min-width: 0;
}

.file-parent-card {
  display: grid;
  width: 194px;
  min-width: 0;
  justify-self: center;
  justify-items: center;
  gap: 14px;
  border-radius: 6px;
  padding: 12px 10px 10px;
  background: transparent;
  color: var(--text);
  text-align: center;
  transition: background 100ms ease;
}

.file-parent-card:hover {
  background: var(--btn-hover);
}

.file-parent-card-frame {
  position: relative;
  display: grid;
  width: 166px;
  height: 118px;
  place-items: center;
  color: var(--folder-icon);
  background: color-mix(in srgb, var(--folder-icon) 8%, transparent);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--folder-icon) 14%, transparent);
}

.file-parent-card-arrow {
  position: absolute;
  right: 48px;
  bottom: 32px;
  border-radius: 999px;
  padding: 2px;
  background: var(--pane-glass);
  color: var(--accent);
  transform: rotate(180deg);
}

.file-parent-card-name {
  max-width: 160px;
  overflow: hidden;
  border-radius: 5px;
  padding: 2px 7px 3px;
  color: var(--text);
  font-size: 14px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-grid-item.file-drop-target :deep(.file-card) {
  background: rgb(var(--accent-rgb) / 0.18);
  box-shadow:
    inset 0 0 0 1px rgb(var(--accent-rgb) / 0.62),
    0 8px 22px rgb(0 0 0 / 0.16);
}

.file-grid-window {
  display: grid;
  align-content: start;
  grid-template-columns: repeat(auto-fill, minmax(206px, 1fr));
  column-gap: 34px;
  row-gap: 38px;
  padding: 36px 30px 52px;
}

.file-grid-empty-message {
  align-self: start;
  grid-column: 1 / -1;
  padding: 2px 8px;
  color: var(--text-muted);
  font-size: 14px;
}

.file-card--loading {
  display: grid;
  width: 194px;
  min-width: 0;
  justify-self: center;
  justify-items: center;
  gap: 14px;
  border-radius: 6px;
  padding: 12px 10px 10px;
}

.loading-card-frame {
  display: block;
  width: 166px;
  height: 118px;
  border: 7px solid rgb(244 243 238 / 0.10);
  box-shadow:
    0 1px 1px rgb(255 255 255 / 0.08),
    0 8px 13px rgb(0 0 0 / 0.18);
}

.file-column-scroller {
  height: 100%;
  min-height: 0;
  overflow: auto;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
  contain: layout paint style;
}

.file-column-track {
  display: flex;
  width: max-content;
  min-width: 100%;
  min-height: 100%;
  align-items: stretch;
}

.file-column {
  width: clamp(212px, 28vw, 292px);
  min-width: 212px;
  padding: 6px 0 18px;
  border-right: 1px solid var(--separator);
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
  outline: 1px solid transparent;
  outline-offset: -1px;
  transition:
    background 100ms ease,
    outline-color 100ms ease;
}

.file-column:first-child {
  background: transparent;
}

.file-column--drop-target {
  background: rgb(var(--accent-rgb) / 0.075);
  outline-color: rgb(var(--accent-rgb) / 0.42);
}

:global(body.is-file-pointer-dragging) .file-column:hover {
  background: rgb(var(--accent-rgb) / 0.055);
  outline-color: rgb(var(--accent-rgb) / 0.34);
}

.file-column-row {
  display: grid;
  width: 100%;
  height: 29px;
  grid-template-columns: 20px minmax(0, 1fr) 16px;
  align-items: center;
  gap: 7px;
  padding: 0 11px 0 16px;
  border-radius: 0;
  background: transparent;
  color: var(--text);
  font-size: 14px;
  font-weight: 400;
  text-align: left;
  transition: background 80ms ease;
}

.file-column-row--directory {
  font-weight: 700;
}

.file-column-row:hover {
  background: var(--btn-hover);
}

.file-column-row--selected {
  background: var(--btn-primary-bg);
  color: white;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.16);
}

.file-column-row--selected:hover {
  background: var(--btn-primary-bg-hover);
}

.file-column-row.file-drop-target {
  background: rgb(var(--accent-rgb) / 0.26);
  box-shadow:
    inset 0 0 0 1px rgb(var(--accent-rgb) / 0.72),
    inset 0 1px 0 rgb(255 255 255 / 0.14);
}

.file-column-glyph {
  display: grid;
  place-items: center;
  color: var(--file-icon);
}

.file-column-glyph--directory {
  color: var(--folder-icon);
}

.file-column-row--selected .file-column-glyph,
.file-column-row--selected .file-column-chevron {
  color: rgb(255 255 255 / 0.88);
}

.file-column-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-column-chevron {
  justify-self: end;
  color: var(--text-faint);
}

.file-column-message {
  margin: 0;
  padding: 12px 16px;
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 600;
}

.file-column-loading {
  display: flex;
  flex-direction: column;
}

.loading-column-row {
  display: grid;
  height: 29px;
  grid-template-columns: 18px minmax(42px, 1fr);
  align-items: center;
  gap: 8px;
  padding: 0 16px;
}

@keyframes loading-sheen {
  0% {
    background-position: 140% 0, 0 0;
  }

  100% {
    background-position: -80% 0, 0 0;
  }
}

@media (prefers-reduced-motion: reduce) {
  .loading-glyph,
  .loading-dot,
  .loading-line,
  .loading-card-frame {
    animation: none;
  }
}

.visually-hidden {
  position: absolute;
  overflow: hidden;
  width: 1px;
  height: 1px;
  clip: rect(0 0 0 0);
  white-space: nowrap;
}
</style>
