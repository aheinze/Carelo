<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import FileEmptyState from './FileEmptyState.vue';
import FileRow from './FileRow.vue';
import { listDirectory } from '../composables/useFileOperations';
import { dropEffectFromEvent, parentPath } from '../composables/useFileTransferGuards';
import { archiveParentPath, archiveRootPath, isArchiveEntry, isArchivePath, isBrowsableEntry } from '../utils/archivePaths';
import { fileTypeIconKind, fileTypeIconName } from '../utils/fileTypeIcons';

const NAME_COLLATOR = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
const SORT_KEYS = ['name', 'extension', 'size', 'modifiedAt', 'none'];
const LIST_ROW_HEIGHT = 29;
const LIST_OVERSCAN_ROWS = 28;
const COLUMN_ROW_HEIGHT = 29;
const COLUMN_OVERSCAN_ROWS = 24;
const GRID_BASE_PADDING_TOP = 36;
const GRID_HORIZONTAL_PADDING = 60;
const GRID_MIN_COLUMN_WIDTH = 206;
const GRID_COLUMN_GAP = 34;
const GRID_ROW_STRIDE = 214;
const GRID_OVERSCAN_ROWS = 3;
const CUSTOM_SCROLLBAR_TRACK_PADDING = 6;
const CUSTOM_SCROLLBAR_MIN_THUMB_SIZE = 40;
const LIST_COLUMN_LIMITS = Object.freeze({
  name: { min: 160, max: 900, default: 180 },
  size: { min: 72, max: 180, default: 88 },
  modifiedAt: { min: 104, max: 280, default: 126 },
});
const LIST_COLUMN_KEY_STEP = 12;
const FILE_DRAG_MIME = 'application/x-carelo-files';
const HANDLED_FILE_DRAG_EVENT = '__careloHandledFileDragEvent';

function fileTypeClass(entry, prefix) {
  return `${prefix}--${fileTypeIconKind(entry)}`;
}

const props = defineProps({
  paneId: {
    type: String,
    default: '',
  },
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
  selectedPaths: {
    type: Array,
    default: () => [],
  },
  loading: {
    type: Boolean,
    required: true,
  },
  loaded: {
    type: Boolean,
    default: false,
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
  columnSelectionResetKey: {
    type: [Number, String],
    default: 0,
  },
  dateFormat: {
    type: String,
    default: 'system',
  },
  alternateRowColors: {
    type: Boolean,
    default: false,
  },
  listColumnWidths: {
    type: Object,
    default: () => ({}),
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
  'list-column-widths-change',
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
const localDraggedPaths = ref([]);
const listScrollTop = ref(0);
const listScrollLeft = ref(0);
const listViewportHeight = ref(0);
const listViewportWidth = ref(0);
const listScrollHeight = ref(0);
const listScrollWidth = ref(0);
const gridScrollTop = ref(0);
const gridViewportHeight = ref(0);
const gridViewportWidth = ref(0);
const gridScrollHeight = ref(0);
const columnScrollTop = ref(0);
const columnScrollLeft = ref(0);
const columnViewportHeight = ref(0);
const columnViewportWidth = ref(0);
const columnScrollHeight = ref(0);
const columnScrollWidth = ref(0);
const customScrollbarDraggingAxis = ref('');
const listColumnWidths = ref(normalizeListColumnWidths());
const resizingListColumn = ref('');
let columnLoadVersion = 0;
let scrollerResizeObserver = null;
let customScrollbarDrag = null;
let listColumnResize = null;

const activeSearchQuery = computed(() => props.searchQuery.trim().toLowerCase());
const isSearchFiltering = computed(() => activeSearchQuery.value.length > 0);
const hasFilteredOutEntries = computed(() =>
  isSearchFiltering.value && props.rawEntryCount > 0 && props.entries.length === 0,
);
const hasHiddenOnlyEntries = computed(() =>
  !isSearchFiltering.value && !props.showHiddenFiles && props.rawEntryCount > 0 && props.entries.length === 0,
);
function emptyStateKind(rawCount, visibleCount) {
  if (isSearchFiltering.value && rawCount > 0 && visibleCount === 0) {
    return 'search';
  }

  if (!isSearchFiltering.value && !props.showHiddenFiles && rawCount > 0 && visibleCount === 0) {
    return 'hidden';
  }

  return 'empty';
}

function emptyStateTitleForKind(kind) {
  if (kind === 'search') {
    return 'No matches';
  }

  if (kind === 'hidden') {
    return 'Only hidden items';
  }

  return 'Nothing here';
}

function emptyStateDetailForKind(kind) {
  if (kind === 'search') {
    return 'Try a different search term.';
  }

  if (kind === 'hidden') {
    return 'Hidden files are available but currently hidden.';
  }

  return 'This folder does not contain any files or folders.';
}

function emptyStateIconForKind(kind) {
  if (kind === 'search') {
    return 'search';
  }

  if (kind === 'hidden') {
    return 'eye-off';
  }

  return 'folder';
}

const emptyStateKindForRoot = computed(() =>
  emptyStateKind(props.rawEntryCount, props.entries.length),
);
const emptyStateTitle = computed(() =>
  emptyStateTitleForKind(emptyStateKindForRoot.value),
);
const emptyStateDetail = computed(() =>
  emptyStateDetailForKind(emptyStateKindForRoot.value),
);
const emptyStateIcon = computed(() =>
  emptyStateIconForKind(emptyStateKindForRoot.value),
);

const baseColumn = computed(() => ({
  path: props.directoryKey,
  title: props.directoryKey,
  entries: props.entries,
  rawEntryCount: props.rawEntryCount,
  selectedIndex: props.selectedIndex,
  loading: props.loading && !props.loaded,
  error: '',
  base: true,
}));

const columns = computed(() => [baseColumn.value, ...columnTrail.value]);
const parentDirectory = computed(() => parentPathForDirectory(props.directoryKey));
const listGridColumns = computed(() => (
  `minmax(${listColumnWidths.value.name}px, 1fr) 46px ${listColumnWidths.value.size}px ${listColumnWidths.value.modifiedAt}px`
));
const listMinimumWidth = computed(() => (
  listColumnWidths.value.name
  + 46
  + listColumnWidths.value.size
  + listColumnWidths.value.modifiedAt
  + (12 * 3)
  + 54
));
const listColumnStyle = computed(() => ({
  '--file-list-grid-columns': listGridColumns.value,
  '--file-list-min-width': `${listMinimumWidth.value}px`,
  '--file-list-scroll-left': `${listScrollLeft.value}px`,
}));
const listStripeOffset = computed(() => (parentDirectory.value ? 1 : 0));
const materializedColumns = computed(() =>
  columns.value.map((column) => ({
    ...column,
    visibleEntries: computeVisibleEntriesForColumn(column),
  })),
);

function virtualRangeForRows(count, scrollTop, viewportHeight, rowHeight, overscanRows) {
  const itemCount = Math.max(0, Number(count) || 0);

  if (itemCount === 0) {
    return {
      start: 0,
      end: 0,
      paddingBefore: 0,
      paddingAfter: 0,
    };
  }

  const safeScrollTop = Math.max(0, Number(scrollTop) || 0);
  const safeViewportHeight = Math.max(Number(viewportHeight) || rowHeight * 20, rowHeight);
  const visibleCount = Math.ceil(safeViewportHeight / rowHeight) + (overscanRows * 2);
  const rawStart = Math.max(0, Math.floor(safeScrollTop / rowHeight) - overscanRows);
  const start = Math.min(rawStart, Math.max(0, itemCount - visibleCount));
  const end = Math.min(itemCount, start + visibleCount);

  return {
    start,
    end,
    paddingBefore: start * rowHeight,
    paddingAfter: Math.max(0, (itemCount - end) * rowHeight),
  };
}

const listVirtualRange = computed(() => {
  const parentOffset = parentDirectory.value ? LIST_ROW_HEIGHT : 0;
  return virtualRangeForRows(
    props.entries.length,
    Math.max(0, listScrollTop.value - parentOffset),
    listViewportHeight.value,
    LIST_ROW_HEIGHT,
    LIST_OVERSCAN_ROWS,
  );
});

const virtualListItems = computed(() =>
  props.entries
    .slice(listVirtualRange.value.start, listVirtualRange.value.end)
    .map((entry, offset) => ({
      entry,
      index: listVirtualRange.value.start + offset,
    })),
);

const gridColumnCount = computed(() => {
  const availableWidth = Math.max(0, (gridViewportWidth.value || 0) - GRID_HORIZONTAL_PADDING);
  return Math.max(
    1,
    Math.floor((availableWidth + GRID_COLUMN_GAP) / (GRID_MIN_COLUMN_WIDTH + GRID_COLUMN_GAP)) || 1,
  );
});

const gridVirtualRange = computed(() => {
  const totalSlots = props.entries.length + (parentDirectory.value ? 1 : 0);
  const columnsPerRow = gridColumnCount.value;
  const totalRows = Math.ceil(totalSlots / columnsPerRow);

  if (totalSlots === 0 || totalRows === 0) {
    return {
      startSlot: 0,
      endSlot: 0,
      paddingBefore: 0,
      paddingAfter: 0,
    };
  }

  const safeScrollTop = Math.max(0, gridScrollTop.value - GRID_BASE_PADDING_TOP);
  const safeViewportHeight = Math.max(gridViewportHeight.value || GRID_ROW_STRIDE * 4, GRID_ROW_STRIDE);
  const visibleRows = Math.ceil(safeViewportHeight / GRID_ROW_STRIDE) + (GRID_OVERSCAN_ROWS * 2);
  const rawStartRow = Math.max(0, Math.floor(safeScrollTop / GRID_ROW_STRIDE) - GRID_OVERSCAN_ROWS);
  const startRow = Math.min(rawStartRow, Math.max(0, totalRows - visibleRows));
  const endRow = Math.min(totalRows, startRow + visibleRows);

  return {
    startSlot: startRow * columnsPerRow,
    endSlot: Math.min(totalSlots, endRow * columnsPerRow),
    paddingBefore: startRow * GRID_ROW_STRIDE,
    paddingAfter: Math.max(0, (totalRows - endRow) * GRID_ROW_STRIDE),
  };
});

const gridWindowStyle = computed(() => ({
  '--virtual-padding-before': `${gridVirtualRange.value.paddingBefore}px`,
  '--virtual-padding-after': `${gridVirtualRange.value.paddingAfter}px`,
}));

const virtualGridSlots = computed(() => {
  const slots = [];
  const hasParent = Boolean(parentDirectory.value);

  for (let slotIndex = gridVirtualRange.value.startSlot; slotIndex < gridVirtualRange.value.endSlot; slotIndex += 1) {
    if (hasParent && slotIndex === 0) {
      slots.push({
        key: 'parent',
        type: 'parent',
      });
      continue;
    }

    const entryIndex = slotIndex - (hasParent ? 1 : 0);
    const entry = props.entries[entryIndex];

    if (entry) {
      slots.push({
        key: entry.path,
        type: 'entry',
        entry,
        index: entryIndex,
      });
    }
  }

  return slots;
});

const virtualColumns = computed(() =>
  materializedColumns.value.map((column) => {
    const range = virtualRangeForRows(
      column.visibleEntries.length,
      Math.max(0, columnScrollTop.value - 6),
      columnViewportHeight.value,
      COLUMN_ROW_HEIGHT,
      COLUMN_OVERSCAN_ROWS,
    );

    return {
      ...column,
      virtualRange: range,
      virtualEntries: column.visibleEntries
        .slice(range.start, range.end)
        .map((entry, offset) => ({
          entry,
          index: range.start + offset,
        })),
    };
  }),
);

function clampNumber(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function normalizeListColumnWidths(widths = props.listColumnWidths) {
  const value = widths && typeof widths === 'object' ? widths : {};

  return Object.fromEntries(
    Object.entries(LIST_COLUMN_LIMITS).map(([key, limits]) => {
      const width = Number(value[key]);
      const normalizedWidth = Number.isFinite(width) ? width : limits.default;

      return [key, Math.round(clampNumber(normalizedWidth, limits.min, limits.max))];
    }),
  );
}

function scrollbarGeometry(scrollOffset, viewportSize, scrollSize, trackSize = viewportSize) {
  const viewport = Math.max(0, Number(viewportSize) || 0);
  const scrollExtent = Math.max(viewport, Number(scrollSize) || 0);
  const maxScroll = Math.max(0, scrollExtent - viewport);

  if (viewport <= 0 || maxScroll <= 1) {
    return null;
  }

  const safeTrackSize = Math.max(0, Number(trackSize) || viewport);
  const usableTrackSize = Math.max(0, safeTrackSize - (CUSTOM_SCROLLBAR_TRACK_PADDING * 2));
  const thumbSize = clampNumber(
    (viewport / scrollExtent) * usableTrackSize,
    Math.min(CUSTOM_SCROLLBAR_MIN_THUMB_SIZE, usableTrackSize),
    usableTrackSize,
  );
  const maxThumbOffset = Math.max(0, usableTrackSize - thumbSize);
  const scrollRatio = maxScroll > 0 ? clampNumber(scrollOffset / maxScroll, 0, 1) : 0;
  const thumbOffset = CUSTOM_SCROLLBAR_TRACK_PADDING + (maxThumbOffset * scrollRatio);

  return {
    maxScroll,
    maxThumbOffset,
    thumbOffset,
    thumbSize,
  };
}

function customScrollbarStyle(scrollOffset, viewportSize, scrollSize) {
  const geometry = scrollbarGeometry(scrollOffset, viewportSize, scrollSize);

  if (!geometry) {
    return null;
  }

  return {
    '--file-scrollbar-thumb-offset': `${geometry.thumbOffset}px`,
    '--file-scrollbar-thumb-size': `${geometry.thumbSize}px`,
  };
}

const verticalScrollbarStyle = computed(() => {
  if (props.loading && !props.loaded) {
    return null;
  }

  if (props.viewMode === 'grid') {
    return customScrollbarStyle(gridScrollTop.value, gridViewportHeight.value, gridScrollHeight.value);
  }

  if (props.viewMode === 'columns') {
    return customScrollbarStyle(columnScrollTop.value, columnViewportHeight.value, columnScrollHeight.value);
  }

  return customScrollbarStyle(listScrollTop.value, listViewportHeight.value, listScrollHeight.value);
});

const horizontalScrollbarStyle = computed(() => {
  if (props.loading && !props.loaded) {
    return null;
  }

  if (props.viewMode === 'list') {
    return customScrollbarStyle(listScrollLeft.value, listViewportWidth.value, listScrollWidth.value);
  }

  if (props.viewMode !== 'columns') {
    return null;
  }

  return customScrollbarStyle(columnScrollLeft.value, columnViewportWidth.value, columnScrollWidth.value);
});

function activeScroller() {
  if (props.viewMode === 'grid') {
    return gridScroller.value;
  }

  if (props.viewMode === 'columns') {
    return columnScroller.value;
  }

  return listScroller.value;
}

function updateListViewport() {
  const scroller = listScroller.value;

  if (!scroller) {
    return;
  }

  listScrollTop.value = scroller.scrollTop;
  listScrollLeft.value = scroller.scrollLeft;
  listViewportHeight.value = scroller.clientHeight;
  listViewportWidth.value = scroller.clientWidth;
  listScrollHeight.value = scroller.scrollHeight;
  listScrollWidth.value = scroller.scrollWidth;
}

function updateGridViewport() {
  const scroller = gridScroller.value;

  if (!scroller) {
    return;
  }

  gridScrollTop.value = scroller.scrollTop;
  gridViewportHeight.value = scroller.clientHeight;
  gridViewportWidth.value = scroller.clientWidth;
  gridScrollHeight.value = scroller.scrollHeight;
}

function updateColumnViewport() {
  const scroller = columnScroller.value;

  if (!scroller) {
    return;
  }

  columnScrollTop.value = scroller.scrollTop;
  columnScrollLeft.value = scroller.scrollLeft;
  columnViewportHeight.value = scroller.clientHeight;
  columnViewportWidth.value = scroller.clientWidth;
  columnScrollHeight.value = scroller.scrollHeight;
  columnScrollWidth.value = scroller.scrollWidth;
}

function updateAllViewports() {
  updateListViewport();
  updateGridViewport();
  updateColumnViewport();
}

function handleListScroll() {
  updateListViewport();
}

function handleGridScroll() {
  updateGridViewport();
}

function handleColumnScroll() {
  updateColumnViewport();
}

function currentScrollbarGeometry(axis, trackSize = null) {
  const scroller = activeScroller();

  if (!scroller) {
    return null;
  }

  const isVertical = axis === 'vertical';
  return {
    scroller,
    geometry: scrollbarGeometry(
      isVertical ? scroller.scrollTop : scroller.scrollLeft,
      isVertical ? scroller.clientHeight : scroller.clientWidth,
      isVertical ? scroller.scrollHeight : scroller.scrollWidth,
      trackSize ?? (isVertical ? scroller.clientHeight : scroller.clientWidth),
    ),
  };
}

function setScrollbarScrollPosition(axis, scroller, pointerPosition, pointerOffset, geometry) {
  if (!geometry || geometry.maxScroll <= 0) {
    return;
  }

  const thumbOffset = clampNumber(
    pointerPosition - pointerOffset,
    CUSTOM_SCROLLBAR_TRACK_PADDING,
    CUSTOM_SCROLLBAR_TRACK_PADDING + geometry.maxThumbOffset,
  );
  const ratio = geometry.maxThumbOffset > 0
    ? (thumbOffset - CUSTOM_SCROLLBAR_TRACK_PADDING) / geometry.maxThumbOffset
    : 0;

  if (axis === 'vertical') {
    scroller.scrollTop = ratio * geometry.maxScroll;
    updateAllViewports();
    return;
  }

  scroller.scrollLeft = ratio * geometry.maxScroll;
  updateAllViewports();
}

function handleCustomScrollbarPointerMove(event) {
  if (!customScrollbarDrag) {
    return;
  }

  event.preventDefault();
  const { axis, pointerOffset, scroller, track } = customScrollbarDrag;
  const rect = track.getBoundingClientRect();
  const pointerPosition = axis === 'vertical'
    ? event.clientY - rect.top
    : event.clientX - rect.left;
  const trackSize = axis === 'vertical' ? rect.height : rect.width;
  const next = currentScrollbarGeometry(axis, trackSize);

  if (!next || next.scroller !== scroller) {
    return;
  }

  setScrollbarScrollPosition(axis, scroller, pointerPosition, pointerOffset, next.geometry);
}

function stopCustomScrollbarDrag() {
  if (!customScrollbarDrag) {
    return;
  }

  customScrollbarDrag = null;
  customScrollbarDraggingAxis.value = '';
  window.removeEventListener('pointermove', handleCustomScrollbarPointerMove);
  window.removeEventListener('pointerup', stopCustomScrollbarDrag);
  window.removeEventListener('pointercancel', stopCustomScrollbarDrag);
}

function updateListColumnWidth(column, width, { emitChange = false } = {}) {
  const limits = LIST_COLUMN_LIMITS[column];

  if (!limits) {
    return;
  }

  const nextWidths = {
    ...listColumnWidths.value,
    [column]: Math.round(clampNumber(Number(width) || limits.default, limits.min, limits.max)),
  };

  listColumnWidths.value = normalizeListColumnWidths(nextWidths);
  nextTick(updateAllViewports);

  if (emitChange) {
    emit('list-column-widths-change', { ...listColumnWidths.value });
  }
}

function handleListColumnResizeMove(event) {
  if (!listColumnResize) {
    return;
  }

  event.preventDefault();
  updateListColumnWidth(
    listColumnResize.column,
    listColumnResize.startWidth + (event.clientX - listColumnResize.startX),
  );
}

function stopListColumnResize() {
  if (!listColumnResize) {
    return;
  }

  listColumnResize = null;
  resizingListColumn.value = '';
  if (typeof document !== 'undefined') {
    document.body?.classList.remove('is-list-column-resizing');
  }
  window.removeEventListener('pointermove', handleListColumnResizeMove);
  window.removeEventListener('pointerup', stopListColumnResize);
  window.removeEventListener('pointercancel', stopListColumnResize);
  emit('list-column-widths-change', { ...listColumnWidths.value });
}

function startListColumnResize(event, column) {
  const limits = LIST_COLUMN_LIMITS[column];

  if (!limits) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();

  listColumnResize = {
    column,
    startX: event.clientX,
    startWidth: listColumnWidths.value[column] || limits.default,
  };
  resizingListColumn.value = column;
  if (typeof document !== 'undefined') {
    document.body?.classList.add('is-list-column-resizing');
  }
  window.addEventListener('pointermove', handleListColumnResizeMove, { passive: false });
  window.addEventListener('pointerup', stopListColumnResize);
  window.addEventListener('pointercancel', stopListColumnResize);
}

function handleListColumnResizeKeydown(event, column) {
  const limits = LIST_COLUMN_LIMITS[column];

  if (!limits) {
    return;
  }

  if (event.key === 'Home') {
    event.preventDefault();
    updateListColumnWidth(column, limits.min, { emitChange: true });
    return;
  }

  if (event.key === 'End') {
    event.preventDefault();
    updateListColumnWidth(column, limits.max, { emitChange: true });
    return;
  }

  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault();
    updateListColumnWidth(column, limits.default, { emitChange: true });
    return;
  }

  const direction = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;

  if (!direction) {
    return;
  }

  event.preventDefault();
  const step = event.shiftKey ? LIST_COLUMN_KEY_STEP * 3 : LIST_COLUMN_KEY_STEP;
  updateListColumnWidth(column, listColumnWidths.value[column] + (direction * step), { emitChange: true });
}

function resetListColumnWidth(column) {
  const limits = LIST_COLUMN_LIMITS[column];

  if (!limits) {
    return;
  }

  updateListColumnWidth(column, limits.default, { emitChange: true });
}

function resizeHandleLabel(column) {
  if (column === 'modifiedAt') return 'Resize Modified column';
  if (column === 'size') return 'Resize Size column';
  return 'Resize Name column';
}

function handleCustomScrollbarPointerDown(event, axis) {
  const track = event.currentTarget;
  const rect = track.getBoundingClientRect();
  const trackSize = axis === 'vertical' ? rect.height : rect.width;
  const current = currentScrollbarGeometry(axis, trackSize);

  if (!current?.geometry) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();

  const pointerPosition = axis === 'vertical'
    ? event.clientY - rect.top
    : event.clientX - rect.left;
  const thumbStart = current.geometry.thumbOffset;
  const thumbEnd = thumbStart + current.geometry.thumbSize;
  const pointerOffset = pointerPosition >= thumbStart && pointerPosition <= thumbEnd
    ? pointerPosition - thumbStart
    : current.geometry.thumbSize / 2;

  setScrollbarScrollPosition(axis, current.scroller, pointerPosition, pointerOffset, current.geometry);
  customScrollbarDrag = {
    axis,
    pointerOffset,
    scroller: current.scroller,
    track,
  };
  customScrollbarDraggingAxis.value = axis;
  window.addEventListener('pointermove', handleCustomScrollbarPointerMove, { passive: false });
  window.addEventListener('pointerup', stopCustomScrollbarDrag);
  window.addEventListener('pointercancel', stopCustomScrollbarDrag);
}

function observeScrollers() {
  nextTick(() => {
    scrollerResizeObserver?.disconnect();

    if (typeof ResizeObserver !== 'undefined') {
      const targets = [listScroller.value, gridScroller.value, columnScroller.value].filter(Boolean);
      scrollerResizeObserver = new ResizeObserver(updateAllViewports);
      targets.forEach((target) => scrollerResizeObserver.observe(target));
    }

    updateAllViewports();
  });
}

function resetScroll() {
  nextTick(() => {
    const scroller = activeScroller();

    if (scroller) {
      scroller.scrollTop = 0;
      scroller.scrollLeft = 0;
    }

    updateAllViewports();
  });
}

function scrollIndexIntoView(scroller, index, rowHeight, offset = 0) {
  if (!scroller || index < 0) {
    return false;
  }

  const itemTop = offset + (index * rowHeight);
  const itemBottom = itemTop + rowHeight;
  const viewportTop = scroller.scrollTop;
  const viewportBottom = viewportTop + scroller.clientHeight;

  if (itemTop < viewportTop) {
    scroller.scrollTop = itemTop;
    return true;
  }

  if (itemBottom > viewportBottom) {
    scroller.scrollTop = Math.max(0, itemBottom - scroller.clientHeight);
    return true;
  }

  return false;
}

function scrollGridIndexIntoView(scroller, index) {
  if (!scroller || index < 0) {
    return false;
  }

  const slotIndex = index + (parentDirectory.value ? 1 : 0);
  const rowIndex = Math.floor(slotIndex / gridColumnCount.value);
  return scrollIndexIntoView(scroller, rowIndex, GRID_ROW_STRIDE, GRID_BASE_PADDING_TOP);
}

function scrollSelectedIntoView() {
  nextTick(() => {
    if (props.selectedIndex < 0) {
      return;
    }

    const scroller = activeScroller();

    if (props.viewMode === 'list') {
      const didScroll = scrollIndexIntoView(
        scroller,
        props.selectedIndex,
        LIST_ROW_HEIGHT,
        parentDirectory.value ? LIST_ROW_HEIGHT : 0,
      );
      if (didScroll) updateListViewport();
    } else if (props.viewMode === 'grid') {
      const didScroll = scrollGridIndexIntoView(scroller, props.selectedIndex);
      if (didScroll) updateGridViewport();
    } else if (props.viewMode === 'columns') {
      const didScroll = scrollIndexIntoView(scroller, props.selectedIndex, COLUMN_ROW_HEIGHT, 6);
      if (didScroll) updateColumnViewport();
    }

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

function computeVisibleEntriesForColumn(column) {
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

function visibleEntriesForColumn(column) {
  return Array.isArray(column?.visibleEntries)
    ? column.visibleEntries
    : computeVisibleEntriesForColumn(column);
}

function emptyStateKindForColumn(column) {
  const rawCount = column.rawEntryCount ?? column.entries?.length ?? 0;
  const visibleCount = visibleEntriesForColumn(column).length;

  return emptyStateKind(rawCount, visibleCount);
}

function emptyStateTitleForColumn(column) {
  return emptyStateTitleForKind(emptyStateKindForColumn(column));
}

function emptyStateDetailForColumn(column) {
  return emptyStateDetailForKind(emptyStateKindForColumn(column));
}

function emptyStateIconForColumn(column) {
  return emptyStateIconForKind(emptyStateKindForColumn(column));
}

function selectedEntriesForColumn(column, columnIndex) {
  const visibleEntries = visibleEntriesForColumn(column);

  if (columnIndex === 0) {
    const selectedPaths = Array.isArray(props.selectedPaths) ? props.selectedPaths : [];

    if (selectedPaths.length > 0) {
      const selectedPathSet = new Set(selectedPaths);
      return visibleEntries.filter((entry) => selectedPathSet.has(entry.path));
    }

    const focusedEntry = visibleEntries[props.selectedIndex];
    return focusedEntry ? [focusedEntry] : [];
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

function clearCurrentColumnSelection() {
  if (columnTrail.value.length === 0) {
    return;
  }

  const currentIndex = columnTrail.value.length - 1;
  const currentColumn = columnTrail.value[currentIndex];
  const hasSelectionState = currentColumn.selectedIndex >= 0
    || currentColumn.selectionAnchorIndex >= 0
    || (currentColumn.selectedPaths || []).length > 0;

  if (!hasSelectionState) {
    return;
  }

  columnTrail.value = [
    ...columnTrail.value.slice(0, currentIndex),
    {
      ...currentColumn,
      selectedIndex: -1,
      selectionAnchorIndex: -1,
      selectedPaths: [],
    },
  ];
}

function activeColumnDirectory() {
  if (props.viewMode !== 'columns') {
    return props.directoryKey;
  }

  return columnTrail.value.at(-1)?.path || props.directoryKey;
}

function childPathForEntry(entry) {
  return entry?.kind === 'directory' ? entry.path : archiveRootPath(entry?.path);
}

function hasEntryForColumnPath(path) {
  const targetPath = cleanPath(path);

  return props.entries.some((entry) =>
    isBrowsableEntry(entry) && cleanPath(childPathForEntry(entry)) === targetPath,
  );
}

function emitActiveDirectory() {
  emit('active-directory', activeColumnDirectory());
}

function activeColumnState() {
  const activeColumnIndex = Math.max(0, materializedColumns.value.length - 1);
  const column = materializedColumns.value[activeColumnIndex];

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
  const column = materializedColumns.value[columnIndex];

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

  const childPath = childPathForEntry(entry);
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
    const column = materializedColumns.value[columnIndex];
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

function isSameParentDrop(targetDirectory, draggedPaths) {
  const targetPath = cleanPath(targetDirectory);

  return draggedPaths.length > 0 &&
    draggedPaths.every((path) => cleanPath(parentPath(path)) === targetPath);
}

function dataTransferTypes(event) {
  return Array.from(event?.dataTransfer?.types || []);
}

function hasCareloFileDrag(event) {
  return dataTransferTypes(event).includes(FILE_DRAG_MIME);
}

function activeDraggedPaths(event = null) {
  if (props.draggedPaths.length > 0) {
    return props.draggedPaths;
  }

  if (localDraggedPaths.value.length > 0) {
    return localDraggedPaths.value;
  }

  return hasCareloFileDrag(event) ? [] : props.draggedPaths;
}

function isFileDragActive(event = null) {
  return props.dragging || localDraggedPaths.value.length > 0 || hasCareloFileDrag(event);
}

function fileDragElementFromEvent(event, selector) {
  const element = event.target?.closest?.(selector);

  if (element && event.currentTarget?.contains?.(element)) {
    return element;
  }

  if (
    typeof event.clientX !== 'number' ||
    typeof event.clientY !== 'number' ||
    typeof document.elementsFromPoint !== 'function'
  ) {
    return null;
  }

  for (const candidate of document.elementsFromPoint(event.clientX, event.clientY)) {
    const match = candidate?.closest?.(selector);

    if (match && event.currentTarget?.contains?.(match)) {
      return match;
    }
  }

  return null;
}

function entryPathName(path) {
  return String(path || '').split('/').filter(Boolean).pop() || String(path || '');
}

function entryForPath(path) {
  if (!path) {
    return null;
  }

  const baseEntry = props.entries.find((entry) => entry.path === path);

  if (baseEntry) {
    return baseEntry;
  }

  for (const column of materializedColumns.value) {
    const columnEntry = column.entries?.find((entry) => entry.path === path);

    if (columnEntry) {
      return columnEntry;
    }
  }

  return null;
}

function entryFromDragElement(element) {
  const path = element?.dataset?.dropEntryPath;

  if (!path) {
    return null;
  }

  return entryForPath(path) || {
    path,
    name: entryPathName(path),
    kind: element.dataset.dropEntryKind || 'file',
  };
}

function indexFromDragElement(element) {
  const index = Number(element?.dataset?.fileIndex);

  return Number.isInteger(index) ? index : null;
}

function columnIndexFromDragElement(element) {
  const index = Number(element?.dataset?.columnIndex);

  return Number.isInteger(index) ? index : 0;
}

function directoryFromDragElement(element) {
  const directoryElement = element?.closest?.('[data-drop-directory-path]');

  return directoryElement?.dataset?.dropDirectoryPath || activeColumnDirectory();
}

function stopDelegatedDragEvent(event) {
  event[HANDLED_FILE_DRAG_EVENT] = true;
}

function canDropOnEntry(entry, event = null) {
  if (!isFileDragActive(event) || entry?.kind !== 'directory' || isArchivePath(entry?.path)) {
    return false;
  }

  const targetPath = cleanPath(entry.path);
  const draggedPaths = activeDraggedPaths(event);

  return !draggedPaths.some((path) => isSameOrChildPath(targetPath, path)) &&
    !isSameParentDrop(targetPath, draggedPaths);
}

function canDropOnDirectory(directoryPath = activeColumnDirectory(), event = null) {
  if (!isFileDragActive(event) || isArchivePath(directoryPath)) {
    return false;
  }

  const targetPath = cleanPath(directoryPath);
  const draggedPaths = activeDraggedPaths(event);

  return !draggedPaths.some((path) => isSameOrChildPath(targetPath, path)) &&
    !isSameParentDrop(targetPath, draggedPaths);
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
  if (event?.[HANDLED_FILE_DRAG_EVENT]) {
    return;
  }

  emitEntryDragStart(entry, index, columnIndex, event, 'drag-start');
}

function handleEntryPointerDown(entry, index, columnIndex, event) {
  let operationEntries = [];

  if (columnIndex !== 0) {
    const column = materializedColumns.value[columnIndex];

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
    const column = materializedColumns.value[columnIndex];
    const isSelected = columnSelectionClass(column, columnIndex, index);

    if (isSelected) {
      operationEntries = selectedEntriesForColumn(column, columnIndex);
    } else {
      columnTrail.value = updateColumnSelection(columnIndex, index, [], index);
      operationEntries = [entry];
      emit('preview-entry', entry);
    }
  }

  if (eventName === 'drag-start') {
    if (operationEntries.length > 0) {
      localDraggedPaths.value = operationEntries.map((item) => item.path).filter(Boolean);
    } else if (props.selectedPaths.includes(entry.path)) {
      localDraggedPaths.value = props.entries
        .filter((item) => props.selectedPaths.includes(item.path))
        .map((item) => item.path);
    } else {
      localDraggedPaths.value = [entry.path];
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
  if (event?.[HANDLED_FILE_DRAG_EVENT]) {
    return true;
  }

  const canDropIntoEntry = canDropOnEntry(entry, event);
  const canDropIntoCurrent = !canDropIntoEntry && canDropOnDirectory(directoryPath, event);

  if (!canDropIntoEntry && !canDropIntoCurrent) {
    return false;
  }

  event.preventDefault();
  event.stopPropagation();
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = dropEffectFromEvent(event);
  }
  entryDropPath.value = canDropIntoEntry ? entry.path : '';
  currentDirectoryDropActive.value = canDropIntoCurrent;
  currentDirectoryDropPath.value = canDropIntoCurrent ? directoryPath : '';
  return true;
}

function handleEntryDragLeave(entry, event) {
  if (event?.[HANDLED_FILE_DRAG_EVENT]) {
    return;
  }

  if (entryDropPath.value !== entry.path || event.currentTarget.contains(event.relatedTarget)) {
    return;
  }

  entryDropPath.value = '';
}

function handleEntryDrop(entry, event, directoryPath = activeColumnDirectory()) {
  if (event?.[HANDLED_FILE_DRAG_EVENT]) {
    return true;
  }

  const canDropIntoEntry = canDropOnEntry(entry, event);
  const canDropIntoCurrent = !canDropIntoEntry && canDropOnDirectory(directoryPath, event);

  if (!canDropIntoEntry && !canDropIntoCurrent) {
    return false;
  }

  event.preventDefault();
  event.stopPropagation();
  clearDropTarget();

  if (canDropIntoEntry) {
    emit('drop-entry', { entry, event });
  } else {
    emit('drop-current', { event, targetDirectory: directoryPath });
  }

  return true;
}

function handleCurrentDirectoryDragOver(event, directoryPath = activeColumnDirectory()) {
  if (event?.[HANDLED_FILE_DRAG_EVENT]) {
    return true;
  }

  if (!canDropOnDirectory(directoryPath, event) || isFileRowTarget(event)) {
    return false;
  }

  event.preventDefault();
  event.stopPropagation();
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = dropEffectFromEvent(event);
  }
  entryDropPath.value = '';
  currentDirectoryDropActive.value = true;
  currentDirectoryDropPath.value = directoryPath;
  return true;
}

function handleCurrentDirectoryDragLeave(event) {
  if (event?.[HANDLED_FILE_DRAG_EVENT]) {
    return;
  }

  if (event.currentTarget.contains(event.relatedTarget)) {
    return;
  }

  currentDirectoryDropActive.value = false;
  currentDirectoryDropPath.value = '';
}

function handleCurrentDirectoryDrop(event, directoryPath = activeColumnDirectory()) {
  if (event?.[HANDLED_FILE_DRAG_EVENT]) {
    return true;
  }

  if (!canDropOnDirectory(directoryPath, event) || isFileRowTarget(event)) {
    return false;
  }

  event.preventDefault();
  event.stopPropagation();
  clearDropTarget();
  emit('drop-current', { event, targetDirectory: directoryPath });
  return true;
}

function isCurrentDirectoryDropTarget(directoryPath) {
  return currentDirectoryDropActive.value
    && cleanPath(currentDirectoryDropPath.value) === cleanPath(directoryPath);
}

function handleDragEnd(event) {
  if (event?.[HANDLED_FILE_DRAG_EVENT]) {
    return;
  }

  clearDropTarget();
  localDraggedPaths.value = [];
  emit('drag-end', event);
}

function handleDelegatedDragStart(event) {
  const sourceElement = fileDragElementFromEvent(event, '[data-file-drag-source="true"]');
  const entry = entryFromDragElement(sourceElement);

  if (!entry) {
    return;
  }

  stopDelegatedDragEvent(event);
  emitEntryDragStart(
    entry,
    indexFromDragElement(sourceElement),
    columnIndexFromDragElement(sourceElement),
    event,
    'drag-start',
  );
}

function handleDelegatedDragOver(event) {
  const entryElement = fileDragElementFromEvent(event, '[data-drop-entry-path]');

  if (entryElement) {
    const entry = entryFromDragElement(entryElement);
    const handled = entry
      ? handleEntryDragOver(entry, event, directoryFromDragElement(entryElement))
      : false;

    if (handled) {
      event[HANDLED_FILE_DRAG_EVENT] = true;
    }

    return;
  }

  const handled = handleCurrentDirectoryDragOver(event, directoryFromDragElement(event.target));

  if (handled) {
    event[HANDLED_FILE_DRAG_EVENT] = true;
  }
}

function handleDelegatedDragLeave(event) {
  if (event.currentTarget.contains(event.relatedTarget)) {
    return;
  }

  clearDropTarget();
}

function handleDelegatedDrop(event) {
  const entryElement = fileDragElementFromEvent(event, '[data-drop-entry-path]');

  if (entryElement) {
    const entry = entryFromDragElement(entryElement);
    const handled = entry
      ? handleEntryDrop(entry, event, directoryFromDragElement(entryElement))
      : false;

    if (handled) {
      event[HANDLED_FILE_DRAG_EVENT] = true;
    }

    return;
  }

  const handled = handleCurrentDirectoryDrop(event, directoryFromDragElement(event.target));

  if (handled) {
    event[HANDLED_FILE_DRAG_EVENT] = true;
  }
}

function handleDelegatedDragEnd(event) {
  if (!fileDragElementFromEvent(event, '[data-file-drag-source="true"]') && !isFileDragActive(event)) {
    return;
  }

  stopDelegatedDragEvent(event);
  clearDropTarget();
  localDraggedPaths.value = [];
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

function handleBackgroundContext(event) {
  if (event.defaultPrevented || isFileRowTarget(event) || event.target?.closest?.('button, input, textarea, select, [contenteditable="true"]')) {
    return;
  }

  event.preventDefault();
  emit('context', {
    target: 'directory',
    targetDirectory: directoryFromDragElement(event.target),
    x: event.clientX,
    y: event.clientY,
  });
}

function syncBaseColumnSelection() {
  if (props.viewMode !== 'columns' || (props.loading && !props.loaded)) {
    return;
  }

  const entry = props.entries[props.selectedIndex];
  const selectedEntries = props.entries.filter((candidate, index) => props.isEntrySelected(index));

  if (props.selectedIndex < 0 && selectedEntries.length === 0) {
    const firstColumnPath = columnTrail.value[0]?.path;

    if (!firstColumnPath || hasEntryForColumnPath(firstColumnPath)) {
      clearCurrentColumnSelection();
      return;
    }

    resetColumnTrail();
    return;
  }

  if (selectedEntries.length !== 1 || !entry || !isBrowsableEntry(entry)) {
    resetColumnTrail();
    return;
  }

  const childPath = childPathForEntry(entry);

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
  const shouldShowLoadingState = columnTrail.value[columnIndex].entries.length === 0;
  columnTrail.value = [
    ...columnTrail.value.slice(0, columnIndex),
    {
      ...columnTrail.value[columnIndex],
      loading: shouldShowLoadingState,
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
        error: columnTrail.value[currentIndex].entries.length > 0
          ? ''
          : error?.message || 'Unable to refresh folder.',
      },
      ...columnTrail.value.slice(currentIndex + 1),
    ];
  }
}

onMounted(() => {
  observeScrollers();
  if (typeof window !== 'undefined') {
    window.addEventListener('resize', updateAllViewports);
  }
});

onBeforeUnmount(() => {
  stopCustomScrollbarDrag();
  stopListColumnResize();
  scrollerResizeObserver?.disconnect();
  scrollerResizeObserver = null;
  if (typeof window !== 'undefined') {
    window.removeEventListener('resize', updateAllViewports);
  }
});

watch(() => props.directoryKey, () => {
  stopCustomScrollbarDrag();
  stopListColumnResize();
  resetColumnTrail();
  resetScroll();
});
watch(() => props.viewMode, () => {
  stopCustomScrollbarDrag();
  stopListColumnResize();
  observeScrollers();
  resetScroll();
});
watch(
  () => props.listColumnWidths,
  (widths) => {
    if (listColumnResize) {
      return;
    }

    listColumnWidths.value = normalizeListColumnWidths(widths);
    nextTick(updateAllViewports);
  },
  { immediate: true, deep: true },
);
watch(
  () => [props.loaded, props.loading, props.entries.length, parentDirectory.value],
  observeScrollers,
  { flush: 'post' },
);
watch(
  () => columnTrail.value
    .map((column) => `${column.path}:${column.loading}:${column.entries?.length ?? 0}`)
    .join('\u0000'),
  () => nextTick(updateAllViewports),
  { flush: 'post' },
);
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
watch(
  () => props.columnSelectionResetKey,
  () => {
    clearCurrentColumnSelection();
    emitColumnSummary();
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
    :data-view-mode="viewMode"
    :data-drop-directory-path="activeColumnDirectory()"
    :aria-busy="loading"
    @dragstart.capture="handleDelegatedDragStart"
    @dragenter.capture="handleDelegatedDragOver"
    @dragover.capture="handleDelegatedDragOver"
    @dragleave.capture="handleDelegatedDragLeave"
    @drop.capture="handleDelegatedDrop"
    @dragend.capture="handleDelegatedDragEnd"
    @contextmenu="handleBackgroundContext"
    @dragover="handleCurrentDirectoryDragOver"
    @dragleave="handleCurrentDirectoryDragLeave"
    @drop="handleCurrentDirectoryDrop"
    @click="handleBackgroundClick"
  >
    <template v-if="loading && !loaded">
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

        <div v-else class="file-list-frame" :style="listColumnStyle" role="table" aria-label="Loading directory listing">
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
      class="file-list-empty-wrap"
    >
      <FileEmptyState
        :icon="emptyStateIcon"
        :title="emptyStateTitle"
        :detail="emptyStateDetail"
      />
    </div>

    <div
      v-else-if="viewMode === 'grid'"
      ref="gridScroller"
      class="file-grid-scroller"
      role="list"
      aria-label="Directory grid"
      :data-drop-directory-path="directoryKey"
      @scroll.passive="handleGridScroll"
    >
      <div class="file-grid-window" :style="gridWindowStyle">
        <template v-for="slot in virtualGridSlots" :key="slot.key">
        <div v-if="slot.type === 'parent'" class="file-grid-item file-parent-grid-item">
          <button
            type="button"
            class="file-parent-card"
            v-tooltip="{ text: `Go to ${parentDirectory}`, shortcut: 'Backspace' }"
            @dblclick="openParentDirectory"
            @keydown.stop
          >
            <span class="file-parent-card-frame" aria-hidden="true">
              <AppIcon name="folder" :size="58" :stroke-width="1.55" />
              <AppIcon class="file-parent-card-arrow" name="chevron-down" :size="18" :stroke-width="2.2" />
            </span>
            <span class="file-parent-card-name">Parent Folder</span>
          </button>
        </div>

        <div
          v-else
          v-memo="[slot.entry.path, slot.entry.name, slot.entry.size, slot.entry.modifiedAt, isEntrySelected(slot.index), entryDropPath === slot.entry.path, dateFormat]"
          class="file-grid-item"
          :class="{ 'file-drop-target': entryDropPath === slot.entry.path }"
          :data-file-index="slot.index"
          data-column-index="0"
          :data-drop-entry-path="slot.entry.path"
          :data-drop-entry-kind="slot.entry.kind"
          data-file-drag-source="true"
          draggable="true"
          @pointerdown="handleEntryPointerDown(slot.entry, slot.index, 0, $event)"
          @dragstart.stop="handleEntryDragStart(slot.entry, slot.index, 0, $event)"
          @dragend="handleDragEnd"
          @dragover="handleEntryDragOver(slot.entry, $event, directoryKey)"
          @dragleave="handleEntryDragLeave(slot.entry, $event)"
          @drop="handleEntryDrop(slot.entry, $event, directoryKey)"
          @contextmenu.prevent="$emit('context', { index: slot.index, entry: slot.entry, x: $event.clientX, y: $event.clientY })"
        >
          <FileRow
            :entry="slot.entry"
            :selected="isEntrySelected(slot.index)"
            :date-format="dateFormat"
            variant="grid"
            draggable
            @click="$emit('select', { index: slot.index, event: $event })"
            @open="$emit('open', slot.index)"
            @drag-start="handleEntryDragStart(slot.entry, slot.index, 0, $event)"
            @drag-end="handleDragEnd"
            @drag-over="handleEntryDragOver(slot.entry, $event, directoryKey)"
            @drag-leave="handleEntryDragLeave(slot.entry, $event)"
            @drop="handleEntryDrop(slot.entry, $event, directoryKey)"
          />
        </div>
        </template>

        <div
          v-if="entries.length === 0 && parentDirectory"
          class="file-grid-empty-message"
        >
          <FileEmptyState
            :icon="emptyStateIcon"
            :title="emptyStateTitle"
            :detail="emptyStateDetail"
            grid
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
      @scroll.passive="handleColumnScroll"
    >
      <div class="file-column-track">
        <section
          v-for="(column, columnIndex) in virtualColumns"
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
          <div v-else-if="column.visibleEntries.length === 0" class="file-column-message file-column-empty">
            <FileEmptyState
              :icon="emptyStateIconForColumn(column)"
              :title="emptyStateTitleForColumn(column)"
              :detail="emptyStateDetailForColumn(column)"
              compact
            />
          </div>

          <template v-else>
            <div
              v-if="column.virtualRange.paddingBefore > 0"
              class="file-column-spacer"
              :style="{ height: `${column.virtualRange.paddingBefore}px` }"
              aria-hidden="true"
            ></div>
            <button
              v-for="item in column.virtualEntries"
              :key="item.entry.path"
              type="button"
              class="file-column-row"
              :class="{
                'file-column-row--striped': alternateRowColors && item.index % 2 === 1,
                'file-column-row--selected': columnSelectionClass(column, columnIndex, item.index),
                'file-column-row--directory': item.entry.kind === 'directory',
                'file-column-row--archive': isArchiveEntry(item.entry),
                'file-drop-target': entryDropPath === item.entry.path,
              }"
              :data-file-index="columnIndex === 0 ? item.index : null"
              :data-column-index="columnIndex"
              :data-drop-entry-path="item.entry.path"
              :data-drop-entry-kind="item.entry.kind"
              data-file-drag-source="true"
              draggable="true"
              :aria-selected="columnSelectionClass(column, columnIndex, item.index)"
              :title="item.entry.name"
              @pointerdown="handleEntryPointerDown(item.entry, item.index, columnIndex, $event)"
              @dragstart.stop="handleEntryDragStart(item.entry, item.index, columnIndex, $event)"
              @dragend="handleDragEnd"
              @dragover="handleEntryDragOver(item.entry, $event, column.path)"
              @dragleave="handleEntryDragLeave(item.entry, $event)"
              @drop="handleEntryDrop(item.entry, $event, column.path)"
              @click="handleColumnSelect(item.entry, item.index, columnIndex, $event)"
              @dblclick="handleColumnOpen(item.entry, item.index, columnIndex)"
              @contextmenu.prevent="handleColumnContext(item.entry, item.index, columnIndex, $event)"
            >
              <span class="file-column-glyph" :class="[ `file-column-glyph--${item.entry.kind}`, fileTypeClass(item.entry, 'file-column-glyph') ]">
                <AppIcon
                  :name="fileTypeIconName(item.entry)"
                  :size="17"
                  :stroke-width="1.8"
                />
              </span>
              <span class="file-column-name">{{ item.entry.name }}</span>
              <AppIcon
                v-if="isBrowsableEntry(item.entry)"
                class="file-column-chevron"
                name="chevron-right"
                :size="14"
                :stroke-width="2.1"
              />
            </button>
            <div
              v-if="column.virtualRange.paddingAfter > 0"
              class="file-column-spacer"
              :style="{ height: `${column.virtualRange.paddingAfter}px` }"
              aria-hidden="true"
            ></div>
          </template>
        </section>
      </div>
    </div>

    <div v-else class="file-list-frame" :style="listColumnStyle" role="table" aria-label="Directory listing">
      <div class="file-list-header" role="row">
        <div
          class="file-list-header-cell"
          role="columnheader"
          :aria-sort="sortKey === 'name' ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'"
        >
          <button
            type="button"
            class="file-list-heading"
            :class="{ 'file-list-heading--active': sortKey === 'name' }"
            @click="$emit('sort', 'name')"
            @keydown.stop
          >
            <span>Name</span>
            <AppIcon
              v-if="sortKey === 'name'"
              :name="sortDirection === 'asc' ? 'arrow-up' : 'arrow-down'"
              :size="12"
              :stroke-width="2.1"
            />
          </button>
          <span
            class="file-list-resize-handle"
            :class="{ 'file-list-resize-handle--active': resizingListColumn === 'name' }"
            role="separator"
            tabindex="0"
            aria-orientation="vertical"
            :aria-label="resizeHandleLabel('name')"
            :aria-valuemin="LIST_COLUMN_LIMITS.name.min"
            :aria-valuemax="LIST_COLUMN_LIMITS.name.max"
            :aria-valuenow="listColumnWidths.name"
            @pointerdown="startListColumnResize($event, 'name')"
            @dblclick.stop="resetListColumnWidth('name')"
            @keydown.stop="handleListColumnResizeKeydown($event, 'name')"
            @click.stop
          />
        </div>
        <span class="file-list-header-cell file-list-header-cell--tag" role="columnheader"></span>
        <div
          class="file-list-header-cell"
          role="columnheader"
          :aria-sort="sortKey === 'size' ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'"
        >
          <button
            type="button"
            class="file-list-heading file-list-heading--end"
            :class="{ 'file-list-heading--active': sortKey === 'size' }"
            @click="$emit('sort', 'size')"
            @keydown.stop
          >
            <span>Size</span>
            <AppIcon
              v-if="sortKey === 'size'"
              :name="sortDirection === 'asc' ? 'arrow-up' : 'arrow-down'"
              :size="12"
              :stroke-width="2.1"
            />
          </button>
          <span
            class="file-list-resize-handle"
            :class="{ 'file-list-resize-handle--active': resizingListColumn === 'size' }"
            role="separator"
            tabindex="0"
            aria-orientation="vertical"
            :aria-label="resizeHandleLabel('size')"
            :aria-valuemin="LIST_COLUMN_LIMITS.size.min"
            :aria-valuemax="LIST_COLUMN_LIMITS.size.max"
            :aria-valuenow="listColumnWidths.size"
            @pointerdown="startListColumnResize($event, 'size')"
            @dblclick.stop="resetListColumnWidth('size')"
            @keydown.stop="handleListColumnResizeKeydown($event, 'size')"
            @click.stop
          />
        </div>
        <div
          class="file-list-header-cell"
          role="columnheader"
          :aria-sort="sortKey === 'modifiedAt' ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'"
        >
          <button
            type="button"
            class="file-list-heading file-list-heading--end"
            :class="{ 'file-list-heading--active': sortKey === 'modifiedAt' }"
            @click="$emit('sort', 'modifiedAt')"
            @keydown.stop
          >
            <span>Modified</span>
            <AppIcon
              v-if="sortKey === 'modifiedAt'"
              :name="sortDirection === 'asc' ? 'arrow-up' : 'arrow-down'"
              :size="12"
              :stroke-width="2.1"
            />
          </button>
          <span
            class="file-list-resize-handle file-list-resize-handle--edge"
            :class="{ 'file-list-resize-handle--active': resizingListColumn === 'modifiedAt' }"
            role="separator"
            tabindex="0"
            aria-orientation="vertical"
            :aria-label="resizeHandleLabel('modifiedAt')"
            :aria-valuemin="LIST_COLUMN_LIMITS.modifiedAt.min"
            :aria-valuemax="LIST_COLUMN_LIMITS.modifiedAt.max"
            :aria-valuenow="listColumnWidths.modifiedAt"
            @pointerdown="startListColumnResize($event, 'modifiedAt')"
            @dblclick.stop="resetListColumnWidth('modifiedAt')"
            @keydown.stop="handleListColumnResizeKeydown($event, 'modifiedAt')"
            @click.stop
          />
        </div>
      </div>

      <div
        ref="listScroller"
        class="file-list"
        :data-drop-directory-path="directoryKey"
        @scroll.passive="handleListScroll"
      >
        <div v-if="parentDirectory" class="file-list-item file-parent-list-item">
          <button
            type="button"
            class="file-parent-row"
            @dblclick="openParentDirectory"
            @keydown.stop
          >
            <span class="file-parent-name" v-tooltip="{ text: `Go to ${parentDirectory}`, shortcut: 'Backspace' }">
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

        <div
          v-if="entries.length === 0 && parentDirectory"
          class="file-list-empty-wrap file-list-empty-wrap--inline"
        >
          <FileEmptyState
            :icon="emptyStateIcon"
            :title="emptyStateTitle"
            :detail="emptyStateDetail"
          />
        </div>

        <div
          v-if="listVirtualRange.paddingBefore > 0"
          class="file-list-spacer"
          :style="{ height: `${listVirtualRange.paddingBefore}px` }"
          aria-hidden="true"
        ></div>

        <div
          v-for="item in virtualListItems"
          :key="item.entry.path"
          v-memo="[item.index, item.entry.path, item.entry.name, item.entry.size, item.entry.modifiedAt, isEntrySelected(item.index), entryDropPath === item.entry.path, dateFormat, alternateRowColors, listStripeOffset]"
          class="file-list-item"
          :class="{ 'file-drop-target': entryDropPath === item.entry.path }"
          :data-file-index="item.index"
          data-column-index="0"
          :data-drop-entry-path="item.entry.path"
          :data-drop-entry-kind="item.entry.kind"
          data-file-drag-source="true"
          draggable="true"
          @pointerdown="handleEntryPointerDown(item.entry, item.index, 0, $event)"
          @dragstart.stop="handleEntryDragStart(item.entry, item.index, 0, $event)"
          @dragend="handleDragEnd"
          @dragover="handleEntryDragOver(item.entry, $event, directoryKey)"
          @dragleave="handleEntryDragLeave(item.entry, $event)"
          @drop="handleEntryDrop(item.entry, $event, directoryKey)"
          @contextmenu.prevent="$emit('context', { index: item.index, entry: item.entry, x: $event.clientX, y: $event.clientY })"
        >
          <FileRow
            :entry="item.entry"
            :selected="isEntrySelected(item.index)"
            :date-format="dateFormat"
            :striped="alternateRowColors && (item.index + listStripeOffset) % 2 === 1"
            variant="list"
            draggable
            @click="$emit('select', { index: item.index, event: $event })"
            @open="$emit('open', item.index)"
            @drag-start="handleEntryDragStart(item.entry, item.index, 0, $event)"
            @drag-end="handleDragEnd"
            @drag-over="handleEntryDragOver(item.entry, $event, directoryKey)"
            @drag-leave="handleEntryDragLeave(item.entry, $event)"
            @drop="handleEntryDrop(item.entry, $event, directoryKey)"
          />
        </div>

        <div
          v-if="listVirtualRange.paddingAfter > 0"
          class="file-list-spacer"
          :style="{ height: `${listVirtualRange.paddingAfter}px` }"
          aria-hidden="true"
        ></div>
      </div>
    </div>

    <div
      v-if="verticalScrollbarStyle"
      class="file-custom-scrollbar file-custom-scrollbar--vertical"
      :class="{ 'file-custom-scrollbar--dragging': customScrollbarDraggingAxis === 'vertical' }"
      :style="verticalScrollbarStyle"
      aria-hidden="true"
      @pointerdown="handleCustomScrollbarPointerDown($event, 'vertical')"
    >
      <span class="file-custom-scrollbar-thumb"></span>
    </div>

    <div
      v-if="horizontalScrollbarStyle"
      class="file-custom-scrollbar file-custom-scrollbar--horizontal"
      :class="{ 'file-custom-scrollbar--dragging': customScrollbarDraggingAxis === 'horizontal' }"
      :style="horizontalScrollbarStyle"
      aria-hidden="true"
      @pointerdown="handleCustomScrollbarPointerDown($event, 'horizontal')"
    >
      <span class="file-custom-scrollbar-thumb"></span>
    </div>
  </div>
</template>

<style scoped>
.file-list-root {
  position: relative;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  outline: 1px solid transparent;
  outline-offset: -1px;
  transition: outline-color 100ms ease, background 100ms ease;
}

.file-list-root--drop-target {
  background: rgb(var(--accent-rgb) / 0.055);
  outline-color: rgb(var(--accent-rgb) / 0.42);
}

.file-custom-scrollbar {
  position: absolute;
  z-index: 5;
  border-radius: 999px;
  opacity: 0;
  pointer-events: auto;
  touch-action: none;
  transition: opacity 120ms ease;
}

.file-list-root:hover .file-custom-scrollbar,
.file-list-root:focus-within .file-custom-scrollbar,
.file-custom-scrollbar--dragging {
  opacity: 1;
}

.file-custom-scrollbar--vertical {
  top: 0;
  right: 2px;
  bottom: 0;
  width: 10px;
}

.file-list-root[data-view-mode="list"] .file-custom-scrollbar--vertical {
  top: 35px;
}

.file-custom-scrollbar--horizontal {
  right: 0;
  bottom: 2px;
  left: 0;
  height: 10px;
}

.file-custom-scrollbar-thumb {
  position: absolute;
  border-radius: 999px;
  background:
    linear-gradient(
      color-mix(in srgb, var(--text) 32%, transparent),
      color-mix(in srgb, var(--text) 22%, transparent)
    );
  box-shadow: inset 0 0 0 1px rgb(255 255 255 / 0.04);
}

.file-custom-scrollbar:hover .file-custom-scrollbar-thumb,
.file-custom-scrollbar--dragging .file-custom-scrollbar-thumb {
  background:
    linear-gradient(
      color-mix(in srgb, var(--text) 46%, transparent),
      color-mix(in srgb, var(--text) 30%, transparent)
    );
}

.file-custom-scrollbar--vertical .file-custom-scrollbar-thumb {
  top: var(--file-scrollbar-thumb-offset);
  right: 2px;
  width: 5px;
  height: var(--file-scrollbar-thumb-size);
}

.file-custom-scrollbar--horizontal .file-custom-scrollbar-thumb {
  bottom: 2px;
  left: var(--file-scrollbar-thumb-offset);
  width: var(--file-scrollbar-thumb-size);
  height: 5px;
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
  overflow: hidden;
  contain: layout paint style;
}

.file-list-header {
  flex: 0 0 auto;
  display: grid;
  width: max(100%, var(--file-list-min-width, 530px));
  grid-template-columns: var(--file-list-grid-columns, minmax(180px, 1fr) 46px 88px 126px);
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
  transform: translateX(calc(var(--file-list-scroll-left, 0px) * -1));
  text-transform: none;
}

.file-list-header-cell {
  position: relative;
  display: flex;
  min-width: 0;
  height: 100%;
  align-items: center;
}

.file-list-header-cell--tag {
  justify-content: center;
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
  width: 100%;
  height: 100%;
  min-width: 0;
  align-items: center;
  gap: 4px;
  padding: 0 6px 0 0;
  background: transparent;
  color: inherit;
  font: inherit;
  letter-spacing: inherit;
  text-align: left;
  text-transform: inherit;
}

.file-list-resize-handle {
  position: absolute;
  z-index: 2;
  top: 5px;
  right: -13px;
  width: 18px;
  height: calc(100% - 10px);
  border-radius: 4px;
  cursor: col-resize;
  touch-action: none;
}

.file-list-resize-handle::after {
  position: absolute;
  top: 3px;
  right: 7px;
  bottom: 3px;
  width: 3px;
  background:
    radial-gradient(circle at center, currentColor 0 1px, transparent 1.2px)
    center top / 3px 5px repeat-y;
  color: color-mix(in srgb, var(--text) 22%, transparent);
  content: "";
  opacity: 0;
  transition: opacity 100ms ease, color 100ms ease;
}

.file-list-header-cell:hover .file-list-resize-handle::after {
  opacity: 0.5;
}

.file-list-resize-handle:hover::after,
.file-list-resize-handle:focus-visible::after,
.file-list-resize-handle--active::after {
  color: var(--accent);
  opacity: 0.85;
}

.file-list-resize-handle:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: -2px;
}

:global(body.is-list-column-resizing),
:global(body.is-list-column-resizing *) {
  cursor: col-resize !important;
  user-select: none;
}

.file-list-heading span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-list-heading--end {
  justify-content: flex-end;
  padding-left: 3px;
  padding-right: 8px;
  text-align: right;
}

.file-list-heading--active {
  color: var(--text-muted);
}

.file-list {
  height: 100%;
  min-height: 0;
  flex: 1 1 auto;
  overflow: auto;
  padding: 6px 0 18px;
  overscroll-behavior: contain;
  scrollbar-gutter: auto;
  scrollbar-width: none;
}

.file-list::-webkit-scrollbar,
.file-grid-scroller::-webkit-scrollbar,
.file-column-scroller::-webkit-scrollbar {
  display: none;
  width: 0;
  height: 0;
}

.file-list--loading {
  display: flex;
  flex-direction: column;
}

.file-list-item {
  height: 29px;
  min-width: var(--file-list-min-width, 530px);
  contain: layout paint style;
}

.file-list-spacer,
.file-column-spacer {
  flex: 0 0 auto;
  pointer-events: none;
}

.file-parent-list-item {
  contain: layout paint style;
}

.file-parent-row {
  display: grid;
  width: 100%;
  height: 29px;
  grid-template-columns: var(--file-list-grid-columns, minmax(180px, 1fr) 46px 88px 126px);
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
  justify-self: start;
  align-items: center;
  gap: 7px;
  padding-right: 6px;
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
  padding-right: 8px;
  padding-left: 3px;
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

.file-list-empty-wrap--inline {
  min-height: min(260px, calc(100vh - 290px));
}

.loading-row {
  display: grid;
  min-width: var(--file-list-min-width, 530px);
  grid-template-columns: var(--file-list-grid-columns, minmax(180px, 1fr) 46px 88px 126px);
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
  scrollbar-gutter: auto;
  scrollbar-width: none;
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
  padding:
    calc(36px + var(--virtual-padding-before, 0px))
    30px
    calc(52px + var(--virtual-padding-after, 0px));
}

.file-grid-empty-message {
  grid-column: 1 / -1;
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
  scrollbar-gutter: auto;
  scrollbar-width: none;
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

.file-column-row--striped {
  background: color-mix(in srgb, var(--text) 3.2%, transparent);
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

.file-column-glyph--archive,
.file-column-glyph--audio,
.file-column-glyph--code,
.file-column-glyph--config,
.file-column-glyph--document,
.file-column-glyph--image,
.file-column-glyph--spreadsheet,
.file-column-glyph--presentation,
.file-column-glyph--video {
  color: color-mix(in srgb, var(--file-icon) 82%, var(--file-type-tint, var(--accent)) 18%);
}

.file-column-glyph--archive,
.file-column-glyph--spreadsheet {
  --file-type-tint: var(--folder-icon);
}

.file-column-glyph--audio,
.file-column-glyph--config,
.file-column-glyph--presentation,
.file-column-glyph--video {
  --file-type-tint: var(--accent-warm);
}

.file-column-glyph--code,
.file-column-glyph--document,
.file-column-glyph--image {
  --file-type-tint: var(--accent);
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

.file-column-empty {
  padding: 0;
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
