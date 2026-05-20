import { computed, ref, watch } from 'vue';
import { defineStore } from 'pinia';
import { listen } from '@tauri-apps/api/event';
import {
  addFavorite as addStoredFavorite,
  cancelFileOperation,
  canUseLocalFileAssets,
  getAppSettings as getStoredAppSettings,
  getHomeDirectory,
  listDirectory,
  listFavorites as listStoredFavorites,
  listVolumes,
  moveFavorite as moveStoredFavorite,
  pauseFileOperation,
  removeFavorite as removeStoredFavorite,
  resumeFileOperation,
  saveAppSettings as saveStoredAppSettings,
} from '../composables/useFileOperations';
import { loadUiSettings, saveUiSettings } from '../composables/useSettings';
import {
  archiveDisplayName,
  archiveParentPath,
  archiveRootPath,
  isArchiveEntry,
  isArchivePath,
} from '../utils/archivePaths';
import { applyColorScheme, normalizeColorScheme } from '../utils/colorSchemes';
import { normalizeDateFormat } from '../utils/dateFormat';

let nextTabId = 1;
let nextTabActivityId = 1;
let nextQueueJobId = 1;
let nextOperationLogId = 1;
const SORT_KEYS = ['name', 'extension', 'size', 'modifiedAt', 'none'];
const SORT_DIRECTIONS = ['asc', 'desc'];
const VIEW_MODES = ['list', 'grid', 'columns'];
const APPEARANCE_MODES = ['system', 'light', 'dark'];
const CUSTOM_TOOL_TARGETS = ['both', 'files', 'folders'];
const NAV_HISTORY_LIMIT = 80;
const INACTIVE_TAB_ENTRY_CACHE_LIMIT = 2;
const LARGE_TAB_ENTRY_CACHE_ENTRY_LIMIT = 1500;
const OPERATION_LOG_LIMIT = 120;
const DEFAULT_APP_SETTINGS = Object.freeze({
  appearanceMode: 'system',
  colorScheme: 'carelo',
  defaultViewMode: 'list',
  dateFormat: 'system',
  showHiddenFiles: false,
  restoreSession: true,
  restoreTerminalPanel: false,
  confirmDelete: true,
  terminalStartsInActiveFolder: true,
  customTools: [],
});
const NAME_COLLATOR = new Intl.Collator(undefined, {
  numeric: true,
  sensitivity: 'base',
});

function normalizeSortKey(sortKey) {
  return SORT_KEYS.includes(sortKey) ? sortKey : 'name';
}

function normalizeSortDirection(direction) {
  return SORT_DIRECTIONS.includes(direction) ? direction : 'asc';
}

function normalizeViewMode(viewMode) {
  return VIEW_MODES.includes(viewMode) ? viewMode : DEFAULT_APP_SETTINGS.defaultViewMode;
}

function normalizeAppearanceMode(mode) {
  return APPEARANCE_MODES.includes(mode) ? mode : DEFAULT_APP_SETTINGS.appearanceMode;
}

function normalizeCustomToolTarget(target) {
  return CUSTOM_TOOL_TARGETS.includes(target) ? target : 'both';
}

function normalizeCustomToolExtensions(extensions) {
  if (Array.isArray(extensions)) {
    return extensions
      .map((extension) => String(extension || '').trim().replace(/^\.+/, '').toLowerCase())
      .filter(Boolean)
      .join(', ');
  }

  return String(extensions || '')
    .split(/[,\s]+/)
    .map((extension) => extension.trim().replace(/^\.+/, '').toLowerCase())
    .filter(Boolean)
    .join(', ');
}

function normalizeCustomTools(tools = []) {
  if (!Array.isArray(tools)) {
    return [];
  }

  const seenIds = new Set();

  return tools
    .filter((tool) => tool && typeof tool === 'object')
    .slice(0, 24)
    .map((tool, index) => {
      const fallbackId = `tool-${index + 1}`;
      const rawId = String(tool.id || fallbackId).trim() || fallbackId;
      let id = rawId.replace(/[^a-zA-Z0-9_-]/g, '-').slice(0, 80) || fallbackId;

      if (seenIds.has(id)) {
        id = `${id}-${index + 1}`;
      }

      seenIds.add(id);

      return {
        id,
        name: String(tool.name || '').trim().slice(0, 80),
        command: String(tool.command || '').trim().slice(0, 600),
        enabled: tool.enabled !== false,
        appliesTo: normalizeCustomToolTarget(tool.appliesTo),
        extensions: normalizeCustomToolExtensions(tool.extensions),
      };
    });
}

function normalizeAppSettings(settings = {}) {
  const value = settings && typeof settings === 'object' ? settings : {};

  return {
    ...DEFAULT_APP_SETTINGS,
    ...value,
    appearanceMode: normalizeAppearanceMode(value.appearanceMode),
    colorScheme: normalizeColorScheme(value.colorScheme),
    defaultViewMode: normalizeViewMode(value.defaultViewMode),
    dateFormat: normalizeDateFormat(value.dateFormat),
    showHiddenFiles: value.showHiddenFiles === true,
    restoreSession: value.restoreSession !== false,
    restoreTerminalPanel: value.restoreTerminalPanel === true,
    confirmDelete: value.confirmDelete !== false,
    terminalStartsInActiveFolder: value.terminalStartsInActiveFolder !== false,
    customTools: normalizeCustomTools(value.customTools),
  };
}

function applyAppearanceMode(mode) {
  if (typeof document === 'undefined') {
    return;
  }

  const normalizedMode = normalizeAppearanceMode(mode);

  if (normalizedMode === 'system') {
    document.documentElement.removeAttribute('data-theme');
  } else {
    document.documentElement.dataset.theme = normalizedMode;
  }
}

function defaultDirectionForSort(sortKey) {
  return ['name', 'extension', 'none'].includes(sortKey) ? 'asc' : 'desc';
}

function touchTabActivity(tab) {
  if (tab) {
    tab.lastActiveAt = nextTabActivityId++;
  }
}

function clearTabEntryCache(tab) {
  if (!tab || tab.loading || tab.entries.length === 0) {
    return;
  }

  tab.entries = [];
  tab.entriesPath = '';
  tab.loaded = false;
  tab.selectedIndex = 0;
  tab.selectionAnchorIndex = 0;
  tab.selectedPaths = [];
  tab.error = '';
}

function trimPaneTabEntryCaches(pane) {
  if (!pane) {
    return;
  }

  const inactiveTabs = pane.tabs
    .filter((tab) => tab.id !== pane.activeTabId && tab.entries.length > 0 && !tab.loading)
    .sort((a, b) => (b.lastActiveAt || 0) - (a.lastActiveAt || 0));

  inactiveTabs.forEach((tab, index) => {
    if (index >= INACTIVE_TAB_ENTRY_CACHE_LIMIT || tab.entries.length > LARGE_TAB_ENTRY_CACHE_ENTRY_LIMIT) {
      clearTabEntryCache(tab);
    }
  });
}

function createTab(
  currentPath,
  viewMode = 'list',
  sortKey = 'name',
  sortDirection = 'asc',
  history = null,
  historyIndex = null,
) {
  const normalizedHistory = Array.isArray(history) && history.length > 0
    ? history.map((path) => String(path || '').trim()).filter(Boolean)
    : [currentPath];
  const normalizedHistoryIndex = Number.isInteger(historyIndex)
    ? Math.min(Math.max(historyIndex, 0), normalizedHistory.length - 1)
    : normalizedHistory.length - 1;
  const resolvedPath = normalizedHistory[normalizedHistoryIndex] || currentPath;

  return {
    id: `tab-${nextTabId++}`,
    currentPath: resolvedPath,
    viewMode,
    sortKey: normalizeSortKey(sortKey),
    sortDirection: normalizeSortDirection(sortDirection),
    history: normalizedHistory,
    historyIndex: normalizedHistoryIndex,
    lastActiveAt: nextTabActivityId++,
    loadVersion: 0,
    entries: [],
    entriesPath: '',
    selectedIndex: 0,
    selectionAnchorIndex: 0,
    selectedPaths: [],
    loading: false,
    loaded: false,
    error: '',
  };
}

function createPane(id, initialTabs, fallbackPath, fallbackViewMode) {
  const tabs = initialTabs.length
    ? initialTabs.map((tab) =>
        createTab(
          tab.path || fallbackPath,
          tab.viewMode || fallbackViewMode,
          tab.sortKey,
          tab.sortDirection,
          tab.history,
          tab.historyIndex,
        ),
      )
    : [createTab(fallbackPath, fallbackViewMode)];

  const activeIndex = Math.min(
    Math.max(Number(initialTabs.activeIndex || 0), 0),
    Math.max(tabs.length - 1, 0),
  );
  const activeTab = tabs[activeIndex] || tabs[0];
  touchTabActivity(activeTab);

  return {
    id,
    activeTabId: activeTab.id,
    tabs,
  };
}

function parentPathFor(path) {
  if (isArchivePath(path)) {
    return archiveParentPath(path);
  }

  const cleanPath = String(path || '').replace(/\/+$/, '');

  if (!cleanPath || cleanPath === '/' || cleanPath === '~') {
    return cleanPath || '~';
  }

  if (cleanPath.startsWith('remote://')) {
    const rest = cleanPath.slice('remote://'.length);
    const slashIndex = rest.indexOf('/');
    const volumeId = slashIndex >= 0 ? rest.slice(0, slashIndex) : rest;
    const objectPath = slashIndex >= 0 ? rest.slice(slashIndex + 1).replace(/\/+$/, '') : '';

    if (!volumeId || !objectPath) {
      return cleanPath;
    }

    const parentIndex = objectPath.lastIndexOf('/');
    return parentIndex < 0
      ? `remote://${volumeId}/`
      : `remote://${volumeId}/${objectPath.slice(0, parentIndex)}`;
  }

  if (!cleanPath.includes('/')) {
    return '~';
  }

  const parent = cleanPath.slice(0, cleanPath.lastIndexOf('/'));
  return parent || '/';
}

function normalizeComparablePath(path) {
  const value = String(path || '').trim();

  if (!value || value === '/' || value === '~') {
    return value || '~';
  }

  if (isArchivePath(value)) {
    return value.endsWith('!/') ? value : value.replace(/\/+$/, '');
  }

  return value.replace(/\/+$/, '');
}

function normalizeError(error) {
  if (typeof error === 'string') {
    return error;
  }

  if (error && typeof error.message === 'string') {
    return error.message;
  }

  return 'Unable to load directory data.';
}

function tabTitleForPath(path) {
  if (isArchivePath(path)) {
    return archiveDisplayName(path) || 'Archive';
  }

  const cleanPath = String(path || '~').replace(/\/+$/, '');

  if (!cleanPath || cleanPath === '~') {
    return 'Home';
  }

  if (cleanPath === '/') {
    return 'Root';
  }

  return cleanPath.split('/').filter(Boolean).at(-1) || cleanPath;
}

function savedTabsFor(settings, paneId, fallbackPath, fallbackViewMode) {
  const tabs = settings[`${paneId}Tabs`];

  if (Array.isArray(tabs) && tabs.length > 0) {
    tabs.activeIndex = settings[`${paneId}ActiveTabIndex`] || 0;
    return tabs;
  }

  const legacyPath = settings[`${paneId}Path`];

  if (legacyPath) {
    return [{ path: legacyPath, viewMode: fallbackViewMode }];
  }

  return [{ path: fallbackPath, viewMode: fallbackViewMode }];
}

function activeTabFromPane(pane) {
  if (!pane) {
    return null;
  }

  return pane.tabs.find((tab) => tab.id === pane.activeTabId) || pane.tabs[0] || null;
}

function visibleEntriesForTab(tab, query, showHidden) {
  const rawEntries = tab?.entries || [];
  const baseEntries = showHidden
    ? rawEntries
    : rawEntries.filter((entry) => !entry.isHidden);

  const entries = query
    ? baseEntries.filter((entry) => entry.name.toLowerCase().includes(query))
    : baseEntries;

  return sortEntries(entries, tab?.sortKey, tab?.sortDirection);
}

function kindRank(entry) {
  switch (entry?.kind) {
    case 'directory':
      return 0;
    case 'file':
      return 1;
    case 'symlink':
      return 2;
    default:
      return 3;
  }
}

function compareNames(a, b) {
  return NAME_COLLATOR.compare(a.name, b.name) || a.name.localeCompare(b.name);
}

function compareOptionalNumber(a, b, fallback = 0) {
  return (a ?? fallback) - (b ?? fallback);
}

function extensionForName(name) {
  const value = String(name || '');
  const dotIndex = value.lastIndexOf('.');

  if (dotIndex <= 0 || dotIndex === value.length - 1) {
    return '';
  }

  return value.slice(dotIndex + 1).toLowerCase();
}

function sortEntries(entries, sortKey = 'name', sortDirection = 'asc') {
  const normalizedKey = normalizeSortKey(sortKey);
  const multiplier = normalizeSortDirection(sortDirection) === 'desc' ? -1 : 1;

  if (normalizedKey === 'none') {
    return [...entries];
  }

  return [...entries].sort((a, b) => {
    const kindOrder = kindRank(a) - kindRank(b);

    if (kindOrder !== 0) {
      return kindOrder;
    }

    let sortOrder = 0;

    if (normalizedKey === 'extension') {
      sortOrder = NAME_COLLATOR.compare(extensionForName(a.name), extensionForName(b.name));
    } else if (normalizedKey === 'size') {
      sortOrder = compareOptionalNumber(a.size, b.size, -1);
    } else if (normalizedKey === 'modifiedAt') {
      sortOrder = compareOptionalNumber(a.modifiedAt, b.modifiedAt, 0);
    } else {
      sortOrder = compareNames(a, b);
    }

    return sortOrder !== 0 ? sortOrder * multiplier : compareNames(a, b);
  });
}

function tabById(pane, tabId) {
  if (!pane) {
    return null;
  }

  return pane.tabs.find((tab) => tab.id === tabId) || null;
}

function serializePane(pane) {
  const activeIndex = Math.max(0, pane.tabs.findIndex((tab) => tab.id === pane.activeTabId));

  return {
    activeIndex,
    tabs: pane.tabs.map((tab) => ({
      path: tab.currentPath,
      viewMode: tab.viewMode,
      sortKey: tab.sortKey,
      sortDirection: tab.sortDirection,
      history: tab.history,
      historyIndex: tab.historyIndex,
    })),
  };
}

function canGoBackInTab(tab) {
  return Boolean(tab && tab.historyIndex > 0);
}

function canGoForwardInTab(tab) {
  return Boolean(tab && tab.historyIndex < tab.history.length - 1);
}

function clampIndex(index, min, max) {
  return Math.max(min, Math.min(max, Number(index) || 0));
}

export const useFileManagerStore = defineStore('file-manager', () => {
  const savedSettings = loadUiSettings();
  const initialAppSettings = normalizeAppSettings({
    ...(savedSettings.appSettings || {}),
    showHiddenFiles: savedSettings.appSettings?.showHiddenFiles ?? savedSettings.showHiddenFiles ?? false,
  });
  const appSettings = ref(initialAppSettings);
  const shouldRestoreSession = appSettings.value.restoreSession;
  const defaultViewMode = appSettings.value.defaultViewMode;
  const restoredPaneSettings = shouldRestoreSession ? savedSettings : {};

  applyAppearanceMode(appSettings.value.appearanceMode);
  applyColorScheme(appSettings.value.colorScheme);

  const panes = ref({
    left: createPane(
      'left',
      savedTabsFor(restoredPaneSettings, 'left', restoredPaneSettings.leftPath || '~', defaultViewMode),
      restoredPaneSettings.leftPath || '~',
      defaultViewMode,
    ),
    right: createPane(
      'right',
      savedTabsFor(restoredPaneSettings, 'right', restoredPaneSettings.rightPath || '~', defaultViewMode),
      restoredPaneSettings.rightPath || '~',
      defaultViewMode,
    ),
  });

  const activePaneId = ref('right');
  const sidebarVisible = ref(savedSettings.sidebarVisible ?? true);
  const sidebarWidth = ref(savedSettings.sidebarWidth ?? 310);
  const previewPanelVisible = ref(savedSettings.previewPanelVisible ?? true);
  const previewPanelWidth = ref(savedSettings.previewPanelWidth ?? 340);
  const paneSplitPercent = ref(savedSettings.paneSplitPercent ?? 48);
  const terminalPanelVisible = ref(
    appSettings.value.restoreTerminalPanel ? (savedSettings.terminalPanelVisible ?? false) : false,
  );
  const terminalPanelHeight = ref(savedSettings.terminalPanelHeight ?? 280);
  const showHiddenFiles = ref(appSettings.value.showHiddenFiles);
  const settingsVisible = ref(false);
  const fileSearchVisible = ref(false);
  const fileSearchMode = ref('files');
  const searchQuery = ref('');
  const queue = ref([]);
  const operationLog = ref([]);
  const volumes = ref([]);
  const favorites = ref([]);
  const columnPreviewEntries = ref({ left: null, right: null });
  const columnSelectionStates = ref({ left: null, right: null });
  const columnTargetDirectories = ref({ left: null, right: null });
  const columnRefreshRequests = ref({ left: null, right: null });
  const dragOperation = ref(null);
  let initializePromise = null;
  let stopOperationProgressListener = null;
  let nextDirectoryRefreshId = 1;

  const sidebarSections = computed(() => {
    const sections = [
      {
        title: 'Locations',
        items: [
          {
            name: 'File System',
            path: '/',
            detail: 'Root',
            icon: 'drive',
            color: '#8E8E93',
          },
        ],
      },
      {
        title: 'Favorites',
        items: favorites.value.map((favorite) => ({
          ...favorite,
          icon: favorite.icon || 'folder',
          color: favorite.color || '#5ca8ff',
          isFavorite: true,
          matchPrefix: true,
        })),
      },
    ];

    sections.splice(1, 0, {
      title: 'Devices',
      items: volumes.value.map((volume) => {
        const isRemote = volume.path?.startsWith('remote://');
        const isMountable = !volume.isMounted && Boolean(volume.devicePath);

        return {
          name: volume.name,
          path: volume.path,
          devicePath: volume.devicePath,
          detail: volume.detail || (isRemote ? 'Remote' : 'Mounted'),
          icon: isRemote ? 'network' : 'drive',
          color: isRemote ? '#5e5ce6' : volume.isRemovable ? '#5ca8ff' : '#8E8E93',
          disabled: !volume.isMounted && !isMountable,
          isMountable,
          isRemote,
          matchPrefix: true,
        };
      }),
    });

    return sections;
  });

  const activePane = computed(() => activeTabFor(activePaneId.value));
  const canGoBack = computed(() => canGoBackInTab(activePane.value));
  const canGoForward = computed(() => canGoForwardInTab(activePane.value));
  const visibleEntriesByPane = computed(() => {
    const query = searchQuery.value.trim().toLowerCase();

    return Object.fromEntries(
      Object.entries(panes.value).map(([paneId, pane]) => {
        const tab = activeTabFromPane(pane);
        const entries = visibleEntriesForTab(tab, query, showHiddenFiles.value);

        return [paneId, entries];
      }),
    );
  });

  const persistedPaneState = computed(() => ({
    left: serializePane(panes.value.left),
    right: serializePane(panes.value.right),
  }));

  async function initialize() {
    if (initializePromise) {
      return initializePromise;
    }

    initializePromise = (async () => {
      await loadAppSettings();
      await initializeOperationProgressListener();
      await Promise.all([
        loadFavorites(),
        refreshVolumes(),
      ]);

      if (
        shouldRestoreSession &&
        (
          savedSettings.leftPath ||
          savedSettings.rightPath ||
          savedSettings.leftTabs ||
          savedSettings.rightTabs
        )
      ) {
        return;
      }

      try {
        const home = await getHomeDirectory();
        for (const paneId of ['left', 'right']) {
          const tab = activeTabFor(paneId);
          tab.currentPath = home;
          tab.history = [home];
          tab.historyIndex = 0;
        }
      } catch {
        for (const paneId of ['left', 'right']) {
          const tab = activeTabFor(paneId);
          tab.currentPath = '~';
          tab.history = ['~'];
          tab.historyIndex = 0;
        }
      }
    })();

    return initializePromise;
  }

  async function loadAppSettings() {
    try {
      const storedSettings = await getStoredAppSettings();

      if (storedSettings && typeof storedSettings === 'object') {
        appSettings.value = normalizeAppSettings({
          ...appSettings.value,
          ...storedSettings,
        });
        showHiddenFiles.value = appSettings.value.showHiddenFiles;
      } else {
        saveStoredAppSettings(appSettings.value).catch(() => {});
      }
    } catch {
      // Browser fallback: localStorage settings are loaded synchronously above.
    }
  }

  async function initializeOperationProgressListener() {
    if (stopOperationProgressListener || !canUseLocalFileAssets()) {
      return;
    }

    try {
      stopOperationProgressListener = await listen('file-operation-progress', (event) => {
        updateQueueJobFromProgress(event.payload || {});
      });
    } catch {
      stopOperationProgressListener = null;
    }
  }

  async function refreshVolumes() {
    try {
      volumes.value = await listVolumes();
    } catch {
      volumes.value = [];
    }
  }

  async function loadFavorites() {
    try {
      favorites.value = await listStoredFavorites();
    } catch {
      favorites.value = [];
    }
  }

  function activeTabFor(paneId) {
    return activeTabFromPane(panes.value[paneId]);
  }

  function parentDirectoryFor(path) {
    return parentPathFor(path);
  }

  function effectiveDirectoryFor(paneId) {
    const tab = activeTabFor(paneId);

    if (!tab) {
      return '';
    }

    if (tab.viewMode === 'columns') {
      return columnTargetDirectories.value[paneId] || tab.currentPath;
    }

    return tab.currentPath;
  }

  function tabTitle(tab) {
    return tabTitleForPath(tab?.currentPath);
  }

  function visibleEntriesFor(paneId) {
    return visibleEntriesByPane.value[paneId] || [];
  }

  function selectedEntryFor(paneId) {
    const entries = visibleEntriesFor(paneId);
    const tab = activeTabFor(paneId);
    const columnPreviewEntry = columnPreviewEntries.value[paneId];
    const columnSelectionState = columnSelectionStates.value[paneId];

    if (tab?.viewMode === 'columns' && columnPreviewEntry) {
      return columnPreviewEntry;
    }

    if (tab?.viewMode === 'columns') {
      return columnSelectionState?.focusedEntry
        || columnSelectionState?.entries?.[0]
        || null;
    }

    if (!tab || tab.selectedIndex < 0) {
      return null;
    }

    return entries[tab.selectedIndex] ?? entries[0] ?? null;
  }

  function selectedEntriesFor(paneId) {
    const tab = activeTabFor(paneId);
    const columnSelectionState = columnSelectionStates.value[paneId];

    if (tab?.viewMode === 'columns' && Array.isArray(columnSelectionState?.entries)) {
      return columnSelectionState.entries;
    }

    const selectedPaths = new Set(tab?.selectedPaths || []);

    if (selectedPaths.size === 0) {
      return [];
    }

    return visibleEntriesFor(paneId).filter((entry) => selectedPaths.has(entry.path));
  }

  function operationEntriesFor(paneId) {
    const selectedEntries = selectedEntriesFor(paneId);

    if (selectedEntries.length > 0) {
      return selectedEntries;
    }

    const focusedEntry = selectedEntryFor(paneId);
    return focusedEntry ? [focusedEntry] : [];
  }

  function isEntrySelected(paneId, index) {
    const tab = activeTabFor(paneId);
    const entry = visibleEntriesFor(paneId)[index];

    if (!tab || !entry) {
      return false;
    }

    if (tab.selectedPaths.length > 0) {
      return tab.selectedPaths.includes(entry.path);
    }

    return tab.selectedIndex === index;
  }

  function updateSelectedIndexAfterSort(tab, previousPath) {
    const entries = visibleEntriesForTab(
      tab,
      searchQuery.value.trim().toLowerCase(),
      showHiddenFiles.value,
    );
    const selectedIndex = previousPath
      ? entries.findIndex((entry) => entry.path === previousPath)
      : -1;

    tab.selectedIndex = selectedIndex >= 0 ? selectedIndex : entries.length > 0 ? 0 : -1;
    tab.selectionAnchorIndex = tab.selectedIndex;
  }

  async function loadPane(paneId, tabId = null) {
    const pane = panes.value[paneId];
    const tab = tabId ? tabById(pane, tabId) : activeTabFromPane(pane);

    if (!tab) {
      return;
    }

    const requestedPath = tab.currentPath;
    const query = searchQuery.value.trim().toLowerCase();
    const isRefreshingLoadedPath = tab.loaded && tab.entriesPath === requestedPath;
    const focusedPath = isRefreshingLoadedPath
      ? visibleEntriesForTab(tab, query, showHiddenFiles.value)[tab.selectedIndex]?.path || ''
      : '';

    tab.loading = true;
    tab.error = '';
    clearColumnPreviewEntry(paneId);
    clearColumnSelectionState(paneId);
    clearColumnTargetDirectory(paneId);

    if (!isRefreshingLoadedPath) {
      tab.entries = [];
      tab.entriesPath = '';
      tab.loaded = false;
      tab.selectedIndex = 0;
      tab.selectionAnchorIndex = 0;
      tab.selectedPaths = [];
    }

    const loadVersion = tab.loadVersion + 1;
    tab.loadVersion = loadVersion;

    try {
      const entries = await listDirectory(requestedPath);

      if (tab.loadVersion !== loadVersion) {
        return;
      }

      tab.entries = entries;
      tab.entriesPath = requestedPath;
      tab.loaded = true;
      tab.selectedPaths = tab.selectedPaths.filter((path) =>
        entries.some((entry) => entry.path === path),
      );
      const visibleEntries = visibleEntriesForTab(tab, query, showHiddenFiles.value);
      const focusedIndex = focusedPath
        ? visibleEntries.findIndex((entry) => entry.path === focusedPath)
        : -1;
      tab.selectedIndex = focusedIndex >= 0 ? focusedIndex : visibleEntries.length > 0 ? 0 : -1;
      tab.selectionAnchorIndex = tab.selectedIndex;
    } catch (error) {
      if (tab.loadVersion !== loadVersion) {
        return;
      }

      tab.entries = [];
      tab.entriesPath = requestedPath;
      tab.selectedIndex = -1;
      tab.selectionAnchorIndex = -1;
      tab.loaded = true;
      tab.error = normalizeError(error);
      addOperationLog({
        operation: 'directory',
        label: `Unable to load ${tabTitleForPath(tab.currentPath)}`,
        detail: tab.error,
        status: 'failed',
        path: tab.currentPath,
      });
    } finally {
      if (tab.loadVersion === loadVersion) {
        tab.loading = false;
        trimPaneTabEntryCaches(pane);
      }
    }
  }

  function requestColumnDirectoryRefresh(paneId, path) {
    if (!panes.value[paneId]) {
      return;
    }

    const nextPath = String(path || '').trim();

    if (!nextPath) {
      return;
    }

    columnRefreshRequests.value = {
      ...columnRefreshRequests.value,
      [paneId]: {
        id: `refresh-${Date.now()}-${nextDirectoryRefreshId++}`,
        path: nextPath,
      },
    };
  }

  async function reloadDirectoryInPanes(path, paneIds = null) {
    const normalizedPath = normalizeComparablePath(path);
    const targetPaneIds = Array.isArray(paneIds) && paneIds.length > 0
      ? [...new Set(paneIds.filter((paneId) => panes.value[paneId]))]
      : Object.keys(panes.value);
    const reloads = [];

    for (const paneId of targetPaneIds) {
      const pane = panes.value[paneId];
      requestColumnDirectoryRefresh(paneId, path);

      for (const tab of pane.tabs) {
        if (normalizeComparablePath(tab.currentPath) === normalizedPath) {
          reloads.push(loadPane(paneId, tab.id));
        }
      }
    }

    await Promise.all(reloads);
  }

  function addOperationLog(entry = {}) {
    const id = `log-${Date.now()}-${nextOperationLogId++}`;

    operationLog.value = [
      {
        id,
        jobId: entry.jobId || null,
        operation: entry.operation || 'operation',
        label: entry.label || 'File operation',
        detail: entry.detail || '',
        status: entry.status || 'info',
        path: entry.path || '',
        createdAt: entry.createdAt || Date.now(),
      },
      ...operationLog.value,
    ].slice(0, OPERATION_LOG_LIMIT);

    return id;
  }

  function clearOperationLog() {
    operationLog.value = [];
  }

  function recordQueueJob(id, status, detail = '') {
    const job = queue.value.find((candidate) => candidate.id === id);

    addOperationLog({
      jobId: id,
      operation: job?.operation || 'operation',
      label: job?.label || 'File operation',
      detail: detail || job?.detail || '',
      status,
      path: job?.currentPath || '',
    });
  }

  function startQueueJob(options = {}) {
    const id = `job-${Date.now()}-${nextQueueJobId++}`;
    const now = Date.now();
    const job = {
      id,
      operation: options.operation || 'operation',
      label: options.label || 'Working',
      detail: options.detail || '',
      status: 'running',
      cancelable: options.cancelable ?? true,
      pausable: options.pausable ?? true,
      cancelRequested: false,
      pauseRequested: false,
      processedBytes: 0,
      totalBytes: 0,
      processedEntries: 0,
      totalEntries: 0,
      currentBytes: 0,
      currentTotalBytes: 0,
      currentPath: '',
      progress: null,
      currentProgress: null,
      bytesPerSecond: 0,
      etaSeconds: null,
      retryAction: typeof options.retryAction === 'function' ? options.retryAction : null,
      failedItems: [],
      createdAt: now,
      updatedAt: now,
      lastProgressAt: now,
      finishedAt: null,
    };

    queue.value = [
      ...queue.value,
      job,
    ];

    addOperationLog({
      jobId: id,
      operation: job.operation,
      label: job.label,
      detail: job.detail || 'Started',
      status: job.status,
    });

    return id;
  }

  function updateQueueJob(id, patch = {}) {
    if (!id) {
      return;
    }

    queue.value = queue.value.map((job) => {
      if (job.id !== id) {
        return job;
      }

      const previousProcessedBytes = Number(job.processedBytes || 0);
      const previousProgressAt = Number(job.lastProgressAt || job.createdAt || Date.now());
      const now = Date.now();
      const nextJob = {
        ...job,
        ...patch,
        updatedAt: now,
      };

      if (
        ['running', 'cancelling'].includes(nextJob.status) &&
        Number.isFinite(nextJob.processedBytes) &&
        nextJob.processedBytes >= previousProcessedBytes
      ) {
        const deltaBytes = nextJob.processedBytes - previousProcessedBytes;
        const deltaSeconds = Math.max((now - previousProgressAt) / 1000, 0.001);

        if (deltaBytes > 0 && deltaSeconds > 0) {
          const instantSpeed = deltaBytes / deltaSeconds;
          nextJob.bytesPerSecond = job.bytesPerSecond > 0
            ? (job.bytesPerSecond * 0.72) + (instantSpeed * 0.28)
            : instantSpeed;
          nextJob.lastProgressAt = now;
        }
      }

      if (typeof patch.progress === 'number') {
        nextJob.progress = Math.max(0, Math.min(1, patch.progress));
      } else if (nextJob.totalBytes > 0) {
        nextJob.progress = Math.max(0, Math.min(1, nextJob.processedBytes / nextJob.totalBytes));
      } else if (nextJob.totalEntries > 0) {
        nextJob.progress = Math.max(0, Math.min(1, nextJob.processedEntries / nextJob.totalEntries));
      } else if (nextJob.status === 'completed') {
        nextJob.progress = 1;
      } else {
        nextJob.progress = null;
      }

      if (nextJob.currentTotalBytes > 0) {
        nextJob.currentProgress = Math.max(0, Math.min(1, nextJob.currentBytes / nextJob.currentTotalBytes));
      } else {
        nextJob.currentProgress = null;
      }

      if (nextJob.bytesPerSecond > 0 && nextJob.totalBytes > 0 && nextJob.processedBytes < nextJob.totalBytes) {
        nextJob.etaSeconds = Math.max(0, Math.round((nextJob.totalBytes - nextJob.processedBytes) / nextJob.bytesPerSecond));
      } else {
        nextJob.etaSeconds = null;
      }

      return nextJob;
    });
  }

  function updateQueueJobFromProgress(progress = {}) {
    const currentJob = queue.value.find((job) => job.id === progress.jobId);
    const patch = {
      processedBytes: Number(progress.processedBytes || 0),
      totalBytes: Number(progress.totalBytes || 0),
      processedEntries: Number(progress.processedEntries || 0),
      totalEntries: Number(progress.totalEntries || 0),
      currentBytes: Number(progress.currentBytes || 0),
      currentTotalBytes: Number(progress.currentTotalBytes || 0),
      currentPath: progress.currentPath || '',
    };

    if (progress.operation) {
      patch.operation = progress.operation;
    }

    if (!currentJob?.cancelRequested && !currentJob?.pauseRequested) {
      patch.status = progress.status || 'running';
    }

    updateQueueJob(progress.jobId, patch);
  }

  async function cancelQueueJob(id) {
    const job = queue.value.find((candidate) => candidate.id === id);

    if (!job || !job.cancelable || ['completed', 'failed', 'cancelled'].includes(job.status)) {
      return;
    }

    const previousStatus = job.status;
    const previousDetail = job.detail;
    updateQueueJob(id, {
      status: 'cancelling',
      cancelRequested: true,
      detail: 'Cancelling...',
    });

    try {
      await cancelFileOperation(id);
    } catch (error) {
      updateQueueJob(id, {
        status: previousStatus,
        cancelRequested: false,
        detail: previousDetail || error?.message || 'Unable to request cancellation.',
      });
    }
  }

  async function pauseQueueJob(id) {
    const job = queue.value.find((candidate) => candidate.id === id);

    if (!job || !job.pausable || job.status !== 'running') {
      return;
    }

    updateQueueJob(id, {
      status: 'paused',
      pauseRequested: true,
      bytesPerSecond: 0,
      etaSeconds: null,
    });

    try {
      await pauseFileOperation(id);
    } catch (error) {
      updateQueueJob(id, {
        status: 'running',
        pauseRequested: false,
        detail: error?.message || 'Unable to pause operation.',
      });
    }
  }

  async function resumeQueueJob(id) {
    const job = queue.value.find((candidate) => candidate.id === id);

    if (!job || job.status !== 'paused') {
      return;
    }

    updateQueueJob(id, {
      status: 'running',
      pauseRequested: false,
      lastProgressAt: Date.now(),
    });

    try {
      await resumeFileOperation(id);
    } catch (error) {
      updateQueueJob(id, {
        status: 'paused',
        pauseRequested: true,
        detail: error?.message || 'Unable to resume operation.',
      });
    }
  }

  async function retryQueueJob(id) {
    const job = queue.value.find((candidate) => candidate.id === id);

    if (!job || job.status !== 'failed' || typeof job.retryAction !== 'function') {
      return;
    }

    removeQueueJob(id);
    try {
      await job.retryAction();
    } catch {
      // The retried operation owns its failure state and user-facing dialog.
    }
  }

  function completeQueueJob(id, detail = '') {
    updateQueueJob(id, {
      status: 'completed',
      detail,
      progress: 1,
      cancelRequested: false,
      pauseRequested: false,
      finishedAt: Date.now(),
    });
    recordQueueJob(id, 'completed', detail);
    window.setTimeout(() => removeQueueJob(id), 2800);
  }

  function failQueueJob(id, detail = '', options = {}) {
    const job = queue.value.find((candidate) => candidate.id === id);
    const failedItems = Array.isArray(options.failedItems)
      ? options.failedItems
      : job?.currentPath
        ? [{ path: job.currentPath, message: detail }]
        : [];

    updateQueueJob(id, {
      status: 'failed',
      detail,
      failedItems,
      cancelRequested: false,
      pauseRequested: false,
      finishedAt: Date.now(),
    });
    recordQueueJob(id, 'failed', detail);
  }

  function cancelQueueJobDone(id, detail = 'Cancelled') {
    updateQueueJob(id, {
      status: 'cancelled',
      detail,
      cancelRequested: true,
      pauseRequested: false,
      finishedAt: Date.now(),
    });
    recordQueueJob(id, 'cancelled', detail);
    window.setTimeout(() => removeQueueJob(id), 3000);
  }

  function removeQueueJob(id) {
    queue.value = queue.value.filter((job) => job.id !== id);
  }

  function setActivePane(paneId) {
    if (panes.value[paneId]) {
      activePaneId.value = paneId;
    }
  }

  function switchActivePane() {
    activePaneId.value = activePaneId.value === 'left' ? 'right' : 'left';
  }

  function setActiveTab(paneId, tabId) {
    const pane = panes.value[paneId];

    if (!tabById(pane, tabId)) {
      return;
    }

    pane.activeTabId = tabId;
    clearColumnPreviewEntry(paneId);
    clearColumnSelectionState(paneId);
    clearColumnTargetDirectory(paneId);
    setActivePane(paneId);

    const tab = activeTabFromPane(pane);

    touchTabActivity(tab);

    if (tab && !tab.loaded && !tab.loading) {
      loadPane(paneId, tab.id);
    }

    trimPaneTabEntryCaches(pane);
  }

  function addPaneTab(paneId, path = null) {
    const pane = panes.value[paneId];
    const sourceTab = activeTabFromPane(pane);

    if (!pane || !sourceTab) {
      return;
    }

    const tab = createTab(path || sourceTab.currentPath, appSettings.value.defaultViewMode);
    pane.tabs.push(tab);
    pane.activeTabId = tab.id;
    touchTabActivity(tab);
    setActivePane(paneId);
    loadPane(paneId, tab.id);
    trimPaneTabEntryCaches(pane);
  }

  function duplicatePaneTab(paneId, tabId) {
    const pane = panes.value[paneId];
    const sourceTab = tabById(pane, tabId);

    if (!pane || !sourceTab) {
      return null;
    }

    const sourceIndex = pane.tabs.findIndex((tab) => tab.id === tabId);
    const tab = createTab(
      sourceTab.currentPath,
      sourceTab.viewMode,
      sourceTab.sortKey,
      sourceTab.sortDirection,
      [...sourceTab.history],
      sourceTab.historyIndex,
    );
    const insertionIndex = sourceIndex >= 0 ? sourceIndex + 1 : pane.tabs.length;

    pane.tabs.splice(insertionIndex, 0, tab);
    pane.activeTabId = tab.id;
    clearColumnPreviewEntry(paneId);
    clearColumnSelectionState(paneId);
    clearColumnTargetDirectory(paneId);
    touchTabActivity(tab);
    setActivePane(paneId);
    loadPane(paneId, tab.id);
    trimPaneTabEntryCaches(pane);

    return tab.id;
  }

  function closePaneTab(paneId, tabId) {
    const pane = panes.value[paneId];

    if (!pane || pane.tabs.length <= 1) {
      return;
    }

    const index = pane.tabs.findIndex((tab) => tab.id === tabId);

    if (index < 0) {
      return;
    }

    const wasActive = pane.activeTabId === tabId;
    pane.tabs.splice(index, 1);

    if (wasActive) {
      const nextTab = pane.tabs[Math.max(0, index - 1)] || pane.tabs[0];
      pane.activeTabId = nextTab.id;
      touchTabActivity(nextTab);
      setActivePane(paneId);

      if (!nextTab.loaded && !nextTab.loading) {
        loadPane(paneId, nextTab.id);
      }
    }

    trimPaneTabEntryCaches(pane);
  }

  function closeOtherPaneTabs(paneId, tabId) {
    const pane = panes.value[paneId];
    const tab = tabById(pane, tabId);

    if (!pane || !tab || pane.tabs.length <= 1) {
      return false;
    }

    pane.tabs = [tab];
    pane.activeTabId = tab.id;
    touchTabActivity(tab);
    clearColumnPreviewEntry(paneId);
    clearColumnSelectionState(paneId);
    clearColumnTargetDirectory(paneId);
    setActivePane(paneId);

    if (!tab.loaded && !tab.loading) {
      loadPane(paneId, tab.id);
    }

    return true;
  }

  function closeActivePaneTab() {
    const pane = panes.value[activePaneId.value];
    const tab = activeTabFromPane(pane);

    if (pane && tab) {
      closePaneTab(pane.id, tab.id);
    }
  }

  function activateAdjacentTab(paneId, direction) {
    const pane = panes.value[paneId];

    if (!pane || pane.tabs.length === 0) {
      return;
    }

    const currentIndex = Math.max(0, pane.tabs.findIndex((tab) => tab.id === pane.activeTabId));
    const nextIndex = (currentIndex + direction + pane.tabs.length) % pane.tabs.length;
    setActiveTab(paneId, pane.tabs[nextIndex].id);
  }

  function movePaneTab(sourcePaneId, tabId, targetPaneId, targetIndex = null) {
    const sourcePane = panes.value[sourcePaneId];
    const targetPane = panes.value[targetPaneId];

    if (!sourcePane || !targetPane || !tabId) {
      return false;
    }

    const sourceIndex = sourcePane.tabs.findIndex((tab) => tab.id === tabId);

    if (sourceIndex < 0) {
      return false;
    }

    if (sourcePaneId === targetPaneId) {
      const insertionIndex = clampIndex(
        targetIndex ?? sourcePane.tabs.length,
        0,
        sourcePane.tabs.length,
      );
      let nextIndex = insertionIndex;

      if (sourceIndex < insertionIndex) {
        nextIndex -= 1;
      }

      if (nextIndex === sourceIndex) {
        return false;
      }

      const [tab] = sourcePane.tabs.splice(sourceIndex, 1);
      sourcePane.tabs.splice(nextIndex, 0, tab);
      clearColumnPreviewEntry(sourcePaneId);
      clearColumnSelectionState(sourcePaneId);
      setActivePane(sourcePaneId);
      trimPaneTabEntryCaches(sourcePane);
      return true;
    }

    const [tab] = sourcePane.tabs.splice(sourceIndex, 1);

    if (sourcePane.tabs.length === 0) {
      const replacementTab = createTab(
        '~',
        tab.viewMode,
        tab.sortKey,
        tab.sortDirection,
      );

      sourcePane.tabs.push(replacementTab);
      sourcePane.activeTabId = replacementTab.id;
      touchTabActivity(replacementTab);
      loadPane(sourcePaneId, replacementTab.id);
    } else if (sourcePane.activeTabId === tab.id) {
      const nextTab = sourcePane.tabs[Math.min(sourceIndex, sourcePane.tabs.length - 1)];
      sourcePane.activeTabId = nextTab.id;
      touchTabActivity(nextTab);

      if (!nextTab.loaded && !nextTab.loading) {
        loadPane(sourcePaneId, nextTab.id);
      }
    }

    const insertionIndex = clampIndex(
      targetIndex ?? targetPane.tabs.length,
      0,
      targetPane.tabs.length,
    );
    targetPane.tabs.splice(insertionIndex, 0, tab);
    targetPane.activeTabId = tab.id;
    clearColumnPreviewEntry(sourcePaneId);
    clearColumnPreviewEntry(targetPaneId);
    clearColumnSelectionState(sourcePaneId);
    clearColumnSelectionState(targetPaneId);
    clearColumnTargetDirectory(sourcePaneId);
    clearColumnTargetDirectory(targetPaneId);
    touchTabActivity(tab);
    setActivePane(targetPaneId);

    if (!tab.loaded && !tab.loading) {
      loadPane(targetPaneId, tab.id);
    }

    trimPaneTabEntryCaches(sourcePane);
    trimPaneTabEntryCaches(targetPane);
    return true;
  }

  function selectEntry(paneId, index, options = {}) {
    const tab = activeTabFor(paneId);
    const entries = visibleEntriesFor(paneId);

    if (tab) {
      clearColumnPreviewEntry(paneId);
      const nextIndex = Math.min(entries.length - 1, Math.max(-1, index));
      tab.selectedIndex = nextIndex;

      if (!options.keepSelection) {
        tab.selectedPaths = [];
      }

      if (!options.keepAnchor) {
        tab.selectionAnchorIndex = nextIndex;
      }
    }
  }

  function selectEntryRange(paneId, index) {
    const tab = activeTabFor(paneId);
    const entries = visibleEntriesFor(paneId);

    if (!tab || entries.length === 0) {
      return;
    }

    const nextIndex = Math.min(entries.length - 1, Math.max(0, index));
    const hasAnchor = Number.isInteger(tab.selectionAnchorIndex)
      && tab.selectionAnchorIndex >= 0
      && tab.selectionAnchorIndex < entries.length;
    const hasFocusedEntry = Number.isInteger(tab.selectedIndex)
      && tab.selectedIndex >= 0
      && tab.selectedIndex < entries.length;

    if (!hasAnchor && !hasFocusedEntry) {
      selectEntry(paneId, nextIndex);
      return;
    }

    const anchorIndex = hasAnchor
      ? tab.selectionAnchorIndex
      : tab.selectedIndex;
    const start = Math.min(anchorIndex, nextIndex);
    const end = Math.max(anchorIndex, nextIndex);

    clearColumnPreviewEntry(paneId);
    tab.selectedIndex = nextIndex;
    tab.selectedPaths = entries.slice(start, end + 1).map((entry) => entry.path);
  }

  function moveSelection(paneId, delta, options = {}) {
    const tab = activeTabFor(paneId);
    const entries = visibleEntriesFor(paneId);

    if (!tab || entries.length === 0) {
      return;
    }

    const currentIndex = tab.selectedIndex < 0 ? 0 : tab.selectedIndex;
    const nextIndex = Math.min(entries.length - 1, Math.max(0, currentIndex + delta));
    clearColumnPreviewEntry(paneId);

    if (options.extend) {
      selectEntryRange(paneId, nextIndex);
      return;
    }

    tab.selectedIndex = nextIndex;

    if (!options.keepSelection) {
      tab.selectedPaths = [];
    }

    tab.selectionAnchorIndex = nextIndex;
  }

  function pageSelection(paneId, delta, options = {}) {
    moveSelection(paneId, delta * 12, options);
  }

  function selectFirstEntry(paneId, options = {}) {
    const tab = activeTabFor(paneId);

    if (tab && visibleEntriesFor(paneId).length > 0) {
      if (options.extend) {
        selectEntryRange(paneId, 0);
      } else {
        selectEntry(paneId, 0);
      }
    }
  }

  function selectLastEntry(paneId, options = {}) {
    const tab = activeTabFor(paneId);
    const entries = visibleEntriesFor(paneId);

    if (tab && entries.length > 0) {
      if (options.extend) {
        selectEntryRange(paneId, entries.length - 1);
      } else {
        selectEntry(paneId, entries.length - 1);
      }
    }
  }

  function toggleEntrySelection(paneId, index = null, moveAfter = false) {
    const tab = activeTabFor(paneId);
    const entries = visibleEntriesFor(paneId);
    const selectedIndex = index ?? tab?.selectedIndex ?? -1;
    const entry = entries[selectedIndex];

    if (!tab || !entry) {
      return;
    }

    clearColumnPreviewEntry(paneId);

    const selectedPaths = new Set(tab.selectedPaths);
    const implicitEntry = entries[tab.selectedIndex];
    const isImplicitSelection = selectedPaths.size === 0 && tab.selectedIndex === selectedIndex;
    const isSelected = selectedPaths.has(entry.path) || isImplicitSelection;

    if (selectedPaths.size === 0 && implicitEntry && !isImplicitSelection) {
      selectedPaths.add(implicitEntry.path);
    }

    if (isSelected) {
      selectedPaths.delete(entry.path);
    } else {
      selectedPaths.add(entry.path);
    }

    tab.selectedPaths = [...selectedPaths];
    tab.selectedIndex = selectedPaths.size > 0 ? selectedIndex : -1;
    tab.selectionAnchorIndex = selectedIndex;

    if (moveAfter) {
      moveSelection(paneId, 1, { keepSelection: true });
    }
  }

  function selectAllEntries(paneId) {
    const tab = activeTabFor(paneId);

    if (tab) {
      tab.selectedPaths = visibleEntriesFor(paneId).map((entry) => entry.path);
      tab.selectionAnchorIndex = tab.selectedIndex >= 0 ? tab.selectedIndex : 0;
    }
  }

  function clearSelection(paneId) {
    const tab = activeTabFor(paneId);

    if (tab) {
      tab.selectedPaths = [];
      tab.selectionAnchorIndex = tab.selectedIndex;
    }
  }

  function invertSelection(paneId) {
    const tab = activeTabFor(paneId);
    const entries = visibleEntriesFor(paneId);
    const selectedPaths = new Set(tab?.selectedPaths || []);

    if (tab) {
      tab.selectedPaths = entries
        .filter((entry) => !selectedPaths.has(entry.path))
        .map((entry) => entry.path);
      tab.selectionAnchorIndex = tab.selectedIndex >= 0 ? tab.selectedIndex : 0;
    }
  }

  function selectEntriesWithFocusedExtension(paneId, shouldSelect = true) {
    const tab = activeTabFor(paneId);
    const focusedEntry = selectedEntryFor(paneId);
    const extension = extensionForName(focusedEntry?.name);

    if (!tab || !focusedEntry || focusedEntry.kind === 'directory') {
      return;
    }

    const matchingPaths = visibleEntriesFor(paneId)
      .filter((entry) => entry.kind !== 'directory' && extensionForName(entry.name) === extension)
      .map((entry) => entry.path);
    const selectedPaths = new Set(tab.selectedPaths);

    if (shouldSelect) {
      matchingPaths.forEach((path) => selectedPaths.add(path));
    } else {
      matchingPaths.forEach((path) => selectedPaths.delete(path));
    }

    tab.selectedPaths = [...selectedPaths];
    tab.selectionAnchorIndex = tab.selectedIndex >= 0 ? tab.selectedIndex : 0;
  }

  function openEntry(paneId, index) {
    const entry = visibleEntriesFor(paneId)[index];

    if (entry && entry.kind === 'directory') {
      setPanePath(paneId, entry.path);
    } else if (isArchiveEntry(entry)) {
      setPanePath(paneId, archiveRootPath(entry.path));
    }
  }

  function openSelectedEntry(paneId) {
    const tab = activeTabFor(paneId);

    if (tab) {
      openEntry(paneId, tab.selectedIndex);
    }
  }

  function goToParent(paneId) {
    const tab = activeTabFor(paneId);

    if (tab) {
      setPanePath(paneId, parentPathFor(effectiveDirectoryFor(paneId) || tab.currentPath));
    }
  }

  function setPanePath(paneId, path, options = {}) {
    const tab = activeTabFor(paneId);
    const nextPath = String(path || '').trim();

    if (!tab || !nextPath || nextPath === tab.currentPath) {
      return Promise.resolve();
    }

    clearColumnPreviewEntry(paneId);
    clearColumnSelectionState(paneId);
    clearColumnTargetDirectory(paneId);

    if (options.updateHistory !== false) {
      const currentHistoryPath = tab.history[tab.historyIndex];

      if (currentHistoryPath !== tab.currentPath) {
        tab.history = tab.history.slice(0, tab.historyIndex + 1);
        tab.history.push(tab.currentPath);
        tab.historyIndex = tab.history.length - 1;
      }

      tab.history = tab.history.slice(0, tab.historyIndex + 1);
      tab.history.push(nextPath);
      tab.historyIndex = tab.history.length - 1;

      if (tab.history.length > NAV_HISTORY_LIMIT) {
        const trimCount = tab.history.length - NAV_HISTORY_LIMIT;
        tab.history = tab.history.slice(trimCount);
        tab.historyIndex = Math.max(0, tab.historyIndex - trimCount);
      }
    }

    tab.currentPath = nextPath;
    return loadPane(paneId, tab.id);
  }

  async function revealPathInPane(paneId, path, kind = 'file') {
    const targetPath = String(path || '').trim();

    if (!targetPath) {
      return;
    }

    if (kind === 'directory') {
      await setPanePath(paneId, targetPath);
      setActivePane(paneId);
      return;
    }

    const parentPath = parentPathFor(targetPath);

    await setPanePath(paneId, parentPath);
    setActivePane(paneId);

    const index = visibleEntriesFor(paneId).findIndex((entry) => entry.path === targetPath);

    if (index >= 0) {
      selectEntry(paneId, index);
    }
  }

  function goBack(paneId = activePaneId.value) {
    const tab = activeTabFor(paneId);

    if (!canGoBackInTab(tab)) {
      return;
    }

    tab.historyIndex -= 1;
    setPanePath(paneId, tab.history[tab.historyIndex], { updateHistory: false });
  }

  function goForward(paneId = activePaneId.value) {
    const tab = activeTabFor(paneId);

    if (!canGoForwardInTab(tab)) {
      return;
    }

    tab.historyIndex += 1;
    setPanePath(paneId, tab.history[tab.historyIndex], { updateHistory: false });
  }

  function openFocusedDirectoryInOtherPane(sourcePaneId = activePaneId.value) {
    const targetPaneId = sourcePaneId === 'left' ? 'right' : 'left';
    const sourceTab = activeTabFor(sourcePaneId);
    const focusedEntry = selectedEntryFor(sourcePaneId);
    const nextPath = focusedEntry?.kind === 'directory'
      ? focusedEntry.path
      : isArchiveEntry(focusedEntry)
        ? archiveRootPath(focusedEntry.path)
        : effectiveDirectoryFor(sourcePaneId) || sourceTab?.currentPath;

    if (nextPath) {
      setPanePath(targetPaneId, nextPath);
      setActivePane(targetPaneId);
    }
  }

  function swapPanes() {
    const leftPane = panes.value.left;
    panes.value.left = panes.value.right;
    panes.value.right = leftPane;
    panes.value.left.id = 'left';
    panes.value.right.id = 'right';
    activePaneId.value = activePaneId.value === 'left' ? 'right' : 'left';
  }

  function setPaneView(paneId, viewMode) {
    const tab = activeTabFor(paneId);

    if (tab && ['list', 'grid', 'columns'].includes(viewMode)) {
      clearColumnPreviewEntry(paneId);
      clearColumnSelectionState(paneId);
      clearColumnTargetDirectory(paneId);
      tab.viewMode = viewMode;
    }
  }

  function setColumnPreviewEntry(paneId, entry) {
    if (!panes.value[paneId]) {
      return;
    }

    columnPreviewEntries.value = {
      ...columnPreviewEntries.value,
      [paneId]: entry || null,
    };
  }

  function clearColumnPreviewEntry(paneId) {
    if (!panes.value[paneId] || !columnPreviewEntries.value[paneId]) {
      return;
    }

    columnPreviewEntries.value = {
      ...columnPreviewEntries.value,
      [paneId]: null,
    };
  }

  function setColumnSelectionState(paneId, state) {
    if (!panes.value[paneId]) {
      return;
    }

    const nextState = state
      ? {
          path: state.path || '',
          entries: Array.isArray(state.entries) ? state.entries : [],
          focusedEntry: state.focusedEntry || null,
        }
      : null;

    columnSelectionStates.value = {
      ...columnSelectionStates.value,
      [paneId]: nextState,
    };
  }

  function clearColumnSelectionState(paneId) {
    if (!panes.value[paneId] || !columnSelectionStates.value[paneId]) {
      return;
    }

    columnSelectionStates.value = {
      ...columnSelectionStates.value,
      [paneId]: null,
    };
  }

  function setColumnTargetDirectory(paneId, path) {
    if (!panes.value[paneId]) {
      return;
    }

    const nextPath = String(path || '').trim() || null;

    if (columnTargetDirectories.value[paneId] === nextPath) {
      return;
    }

    columnTargetDirectories.value = {
      ...columnTargetDirectories.value,
      [paneId]: nextPath,
    };
  }

  function clearColumnTargetDirectory(paneId) {
    if (!panes.value[paneId] || !columnTargetDirectories.value[paneId]) {
      return;
    }

    columnTargetDirectories.value = {
      ...columnTargetDirectories.value,
      [paneId]: null,
    };
  }

  function startFileDrag(paneId, entries, requestedMode = null) {
    if (!panes.value[paneId] || !Array.isArray(entries) || entries.length === 0) {
      dragOperation.value = null;
      return;
    }

    dragOperation.value = {
      sourcePaneId: paneId,
      requestedMode,
      entries: entries.map((entry) => ({
        name: entry.name,
        path: entry.path,
        kind: entry.kind,
        isSymlink: entry.isSymlink,
      })),
    };
  }

  function setFileDragMode(requestedMode = null) {
    if (!dragOperation.value) {
      return;
    }

    dragOperation.value = {
      ...dragOperation.value,
      requestedMode,
    };
  }

  function clearFileDrag() {
    dragOperation.value = null;
  }

  function isFavoritePath(path) {
    const normalizedPath = normalizeComparablePath(path);

    return favorites.value.some((favorite) =>
      normalizeComparablePath(favorite.path) === normalizedPath,
    );
  }

  function favoriteInputForEntry(entry) {
    return {
      name: entry.name,
      path: entry.path,
      icon: entry.path === '~' ? 'home' : 'folder',
      color: '#5ca8ff',
    };
  }

  async function addFavoritesFromEntries(entries, targetIndex = null) {
    const directories = (entries || []).filter((entry) =>
      entry?.kind === 'directory' && entry.path && !isArchivePath(entry.path),
    );

    if (directories.length === 0) {
      return [];
    }

    const added = [];
    let insertIndex = Number.isInteger(targetIndex) ? targetIndex : favorites.value.length;

    for (const entry of directories) {
      const favorite = await addStoredFavorite(favoriteInputForEntry(entry));
      added.push(favorite);
      favorites.value = await moveStoredFavorite(favorite.id, insertIndex);
      insertIndex += 1;
    }

    return added;
  }

  async function removeFavorite(id) {
    if (!id) {
      return;
    }

    await removeStoredFavorite(id);
    favorites.value = favorites.value.filter((favorite) => favorite.id !== id);
  }

  async function moveFavorite(id, targetIndex) {
    if (!id) {
      return;
    }

    favorites.value = await moveStoredFavorite(id, targetIndex);
  }

  function setPaneSort(paneId, sortKey) {
    const tab = activeTabFor(paneId);
    const normalizedKey = normalizeSortKey(sortKey);

    if (!tab) {
      return;
    }

    clearColumnPreviewEntry(paneId);
    const previousPath = selectedEntryFor(paneId)?.path;
    clearColumnSelectionState(paneId);

    if (tab.sortKey === normalizedKey) {
      tab.sortDirection = tab.sortDirection === 'asc' ? 'desc' : 'asc';
    } else {
      tab.sortKey = normalizedKey;
      tab.sortDirection = defaultDirectionForSort(normalizedKey);
    }

    updateSelectedIndexAfterSort(tab, previousPath);
  }

  function setPaneSortKey(paneId, sortKey) {
    const tab = activeTabFor(paneId);
    const normalizedKey = normalizeSortKey(sortKey);

    if (!tab || tab.sortKey === normalizedKey) {
      return;
    }

    clearColumnPreviewEntry(paneId);
    const previousPath = selectedEntryFor(paneId)?.path;
    clearColumnSelectionState(paneId);
    tab.sortKey = normalizedKey;
    tab.sortDirection = defaultDirectionForSort(normalizedKey);
    updateSelectedIndexAfterSort(tab, previousPath);
  }

  function togglePaneSortDirection(paneId) {
    const tab = activeTabFor(paneId);

    if (!tab) {
      return;
    }

    clearColumnPreviewEntry(paneId);
    const previousPath = selectedEntryFor(paneId)?.path;
    clearColumnSelectionState(paneId);
    tab.sortDirection = tab.sortDirection === 'asc' ? 'desc' : 'asc';
    updateSelectedIndexAfterSort(tab, previousPath);
  }

  function toggleSidebar() {
    sidebarVisible.value = !sidebarVisible.value;
  }

  function togglePreviewPanel() {
    previewPanelVisible.value = !previewPanelVisible.value;
  }

  function toggleTerminalPanel(forceVisible = null) {
    terminalPanelVisible.value = forceVisible ?? !terminalPanelVisible.value;
  }

  function setSidebarWidth(width) {
    sidebarWidth.value = Math.max(280, Math.min(420, Number(width) || 310));
  }

  function setPreviewPanelWidth(width) {
    previewPanelWidth.value = Math.max(340, Math.min(560, Number(width) || 340));
  }

  function setPaneSplitPercent(percent) {
    paneSplitPercent.value = Math.max(28, Math.min(72, Number(percent) || 48));
  }

  function setTerminalPanelHeight(height) {
    terminalPanelHeight.value = Math.max(180, Math.min(560, Number(height) || 280));
  }

  function setShowHiddenFiles(value) {
    const nextValue = Boolean(value);
    showHiddenFiles.value = nextValue;

    if (appSettings.value.showHiddenFiles !== nextValue) {
      appSettings.value = normalizeAppSettings({
        ...appSettings.value,
        showHiddenFiles: nextValue,
      });
    }
  }

  function toggleHiddenFiles() {
    setShowHiddenFiles(!showHiddenFiles.value);
  }

  function setAppSetting(key, value) {
    const nextSettings = normalizeAppSettings({
      ...appSettings.value,
      [key]: value,
    });
    appSettings.value = nextSettings;
  }

  function openSettings() {
    settingsVisible.value = true;
  }

  function closeSettings() {
    settingsVisible.value = false;
  }

  function toggleSettings() {
    settingsVisible.value = !settingsVisible.value;
  }

  function openFileSearch(mode = 'files') {
    fileSearchMode.value = mode === 'content' ? 'content' : 'files';
    fileSearchVisible.value = true;
  }

  function openContentSearch() {
    openFileSearch('content');
  }

  function closeFileSearch() {
    fileSearchVisible.value = false;
  }

  function toggleFileSearch() {
    fileSearchVisible.value = !fileSearchVisible.value;
  }

  watch(
    () => [
      sidebarVisible.value,
      sidebarWidth.value,
      previewPanelVisible.value,
      previewPanelWidth.value,
      paneSplitPercent.value,
      terminalPanelVisible.value,
      terminalPanelHeight.value,
      showHiddenFiles.value,
      persistedPaneState.value,
    ],
    () => {
      saveUiSettings({
        sidebarVisible: sidebarVisible.value,
        sidebarWidth: sidebarWidth.value,
        previewPanelVisible: previewPanelVisible.value,
        previewPanelWidth: previewPanelWidth.value,
        paneSplitPercent: paneSplitPercent.value,
        terminalPanelVisible: terminalPanelVisible.value,
        terminalPanelHeight: terminalPanelHeight.value,
        showHiddenFiles: showHiddenFiles.value,
        leftPath: activeTabFor('left')?.currentPath || '~',
        rightPath: activeTabFor('right')?.currentPath || '~',
        leftActiveTabIndex: persistedPaneState.value.left.activeIndex,
        rightActiveTabIndex: persistedPaneState.value.right.activeIndex,
        leftTabs: persistedPaneState.value.left.tabs,
        rightTabs: persistedPaneState.value.right.tabs,
      });
    },
    { deep: true },
  );

  watch(
    appSettings,
    (settings) => {
      const normalizedSettings = normalizeAppSettings(settings);
      applyAppearanceMode(normalizedSettings.appearanceMode);
      applyColorScheme(normalizedSettings.colorScheme);
      saveUiSettings({ appSettings: normalizedSettings });
      saveStoredAppSettings(normalizedSettings).catch(() => {});
    },
    { deep: true },
  );

  return {
    panes,
    activePaneId,
    activePane,
    canGoBack,
    canGoForward,
    sidebarVisible,
    sidebarWidth,
    previewPanelVisible,
    previewPanelWidth,
    paneSplitPercent,
    terminalPanelVisible,
    terminalPanelHeight,
    showHiddenFiles,
    settingsVisible,
    fileSearchVisible,
    fileSearchMode,
    appSettings,
    searchQuery,
    queue,
    operationLog,
    volumes,
    favorites,
    columnRefreshRequests,
    dragOperation,
    sidebarSections,
    initialize,
    refreshVolumes,
    activeTabFor,
    parentDirectoryFor,
    effectiveDirectoryFor,
    tabTitle,
    visibleEntriesFor,
    selectedEntryFor,
    setColumnPreviewEntry,
    setColumnSelectionState,
    setColumnTargetDirectory,
    loadPane,
    reloadDirectoryInPanes,
    requestColumnDirectoryRefresh,
    startQueueJob,
    addOperationLog,
    clearOperationLog,
    updateQueueJob,
    cancelQueueJob,
    pauseQueueJob,
    resumeQueueJob,
    retryQueueJob,
    completeQueueJob,
    failQueueJob,
    cancelQueueJobDone,
    removeQueueJob,
    setActivePane,
    switchActivePane,
    setActiveTab,
    addPaneTab,
    duplicatePaneTab,
    closePaneTab,
    closeOtherPaneTabs,
    closeActivePaneTab,
    activateAdjacentTab,
    movePaneTab,
    selectEntry,
    selectEntryRange,
    selectedEntriesFor,
    operationEntriesFor,
    isEntrySelected,
    moveSelection,
    pageSelection,
    selectFirstEntry,
    selectLastEntry,
    toggleEntrySelection,
    selectAllEntries,
    clearSelection,
    invertSelection,
    selectEntriesWithFocusedExtension,
    openEntry,
    openSelectedEntry,
    goToParent,
    revealPathInPane,
    goBack,
    goForward,
    openFocusedDirectoryInOtherPane,
    swapPanes,
    setPanePath,
    setPaneView,
    startFileDrag,
    setFileDragMode,
    clearFileDrag,
    isFavoritePath,
    addFavoritesFromEntries,
    removeFavorite,
    moveFavorite,
    setPaneSort,
    setPaneSortKey,
    togglePaneSortDirection,
    setSidebarWidth,
    setPreviewPanelWidth,
    setPaneSplitPercent,
    toggleSidebar,
    togglePreviewPanel,
    toggleTerminalPanel,
    setTerminalPanelHeight,
    setShowHiddenFiles,
    toggleHiddenFiles,
    setAppSetting,
    openSettings,
    closeSettings,
    toggleSettings,
    openFileSearch,
    openContentSearch,
    closeFileSearch,
    toggleFileSearch,
  };
});
