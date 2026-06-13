import { computed, ref, watch } from 'vue';
import { acceptHMRUpdate, defineStore } from 'pinia';
import { listen } from '@tauri-apps/api/event';
import {
  addFavorite as addStoredFavorite,
  addFavoriteGroup as addStoredFavoriteGroup,
  cancelFileOperation,
  canUseLocalFileAssets,
  checkRemoteVolume,
  clearRemotePreviewCache,
  copyItems,
  deleteItems,
  ejectVolume,
  getAppSettings as getStoredAppSettings,
  getHomeDirectory,
  listDirectory,
  listFavoriteGroups as listStoredFavoriteGroups,
  listFavorites as listStoredFavorites,
  listVolumes,
  mountVolume,
  moveFavorite as moveStoredFavorite,
  moveFileTags,
  moveItems,
  removeFavoriteGroup as removeStoredFavoriteGroup,
  pauseFileOperation,
  removeFavorite as removeStoredFavorite,
  renameItem,
  restoreFromTrash,
  resumeFileOperation,
  saveAppSettings as saveStoredAppSettings,
  setActiveRemoteVolumes,
  unlockVolume,
  watchActiveDirectories,
} from '../composables/useFileOperations';
import { useDialog } from '../composables/useDialog';
import { loadUiSettings, saveUiSettings } from '../composables/useSettings';
import {
  archiveDisplayName,
  archiveParentPath,
  archiveRootPath,
  isArchiveEntry,
  isArchivePath,
} from '../utils/archivePaths';
import {
  applyAccentColor,
  applyColorScheme,
  normalizeAccentColor,
  normalizeColorScheme,
} from '../utils/colorSchemes';
import { normalizeDateFormat } from '../utils/dateFormat';

let nextTabId = 1;
let nextTabActivityId = 1;
let nextQueueJobId = 1;
let nextOperationLogId = 1;
let nextHistoryId = 1;
let historyBusy = false;
let remoteHealthRefreshInFlight = false;
let activeRemoteSyncTimer = null;
let lastReportedRemoteVolumeIds = '';
let stopRemoteEditSyncListener = null;
let stopDirectoryWatchListener = null;
let scheduledDirectoryReloadTimer = null;
let scheduledDirectoryReloadPaths = new Set();
let remoteHealthBackoff = new Map();
let activeDirectoryWatchSyncTimer = null;
let lastWatchedDirectoryPaths = '';
const activeLocalVolumeUnlocks = new Map();
const SORT_KEYS = ['name', 'extension', 'size', 'modifiedAt', 'none'];
const SORT_DIRECTIONS = ['asc', 'desc'];
const VIEW_MODES = ['list', 'grid', 'columns'];
const APPEARANCE_MODES = ['system', 'light', 'dark'];
const CUSTOM_TOOL_TARGETS = ['both', 'files', 'folders'];
const DELETE_MODES = ['trash', 'permanent'];
const DEFAULT_FAVORITE_GROUP_ID = 'favorites';
const NAV_HISTORY_LIMIT = 80;
const REMOTE_HEALTH_ACTIVE_REFRESH_INTERVAL_MS = 60_000;
const REMOTE_HEALTH_IDLE_REFRESH_INTERVAL_MS = 5 * 60_000;
const REMOTE_HEALTH_ERROR_BASE_INTERVAL_MS = 2 * 60_000;
const REMOTE_HEALTH_ERROR_MAX_INTERVAL_MS = 15 * 60_000;
const DIRECTORY_RELOAD_BATCH_DELAY_MS = 120;
const INACTIVE_TAB_ENTRY_CACHE_LIMIT = 2;
const LARGE_TAB_ENTRY_CACHE_ENTRY_LIMIT = 1500;
const OPERATION_LOG_LIMIT = 120;
const HISTORY_LIMIT = 50;
const WORKSPACE_LIMIT = 32;
const WORKSPACE_TAB_LIMIT = 64;
const WORKSPACE_NAME_LIMIT = 80;
const LIST_COLUMN_LIMITS = Object.freeze({
  name: { min: 160, max: 900, default: 180 },
  size: { min: 72, max: 180, default: 88 },
  modifiedAt: { min: 104, max: 280, default: 126 },
});
const ACTIVE_QUEUE_STATUSES = new Set(['running', 'paused', 'cancelling']);
const MISSING_PATH_ERROR_PATTERN = /(no such file or directory|not found|os error 2|cannot find the path)/i;
const DEFAULT_APP_SETTINGS = Object.freeze({
  appearanceMode: 'system',
  colorScheme: 'carelo',
  accentColor: '',
  defaultViewMode: 'list',
  dateFormat: 'system',
  showHiddenFiles: false,
  alternateRowColors: true,
  restoreSession: true,
  restoreTerminalPanel: false,
  confirmDelete: true,
  deleteMode: 'trash',
  terminalStartsInActiveFolder: true,
  editorCommand: '',
  customTools: [],
  listColumnWidths: {
    name: LIST_COLUMN_LIMITS.name.default,
    size: LIST_COLUMN_LIMITS.size.default,
    modifiedAt: LIST_COLUMN_LIMITS.modifiedAt.default,
  },
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

function normalizeDeleteMode(mode) {
  return DELETE_MODES.includes(mode) ? mode : DEFAULT_APP_SETTINGS.deleteMode;
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

function normalizeEditorCommand(command) {
  return String(command || '').slice(0, 600);
}

function normalizeListColumnWidths(widths = {}) {
  const value = widths && typeof widths === 'object' ? widths : {};

  return Object.fromEntries(
    Object.entries(LIST_COLUMN_LIMITS).map(([key, limits]) => {
      const width = Number(value[key]);
      const normalizedWidth = Number.isFinite(width) ? width : limits.default;

      return [key, Math.round(Math.max(limits.min, Math.min(limits.max, normalizedWidth)))];
    }),
  );
}

function normalizeAppSettings(settings = {}) {
  const value = settings && typeof settings === 'object' ? settings : {};

  return {
    ...DEFAULT_APP_SETTINGS,
    ...value,
    appearanceMode: normalizeAppearanceMode(value.appearanceMode),
    colorScheme: normalizeColorScheme(value.colorScheme),
    accentColor: normalizeAccentColor(value.accentColor),
    defaultViewMode: normalizeViewMode(value.defaultViewMode),
    dateFormat: normalizeDateFormat(value.dateFormat),
    showHiddenFiles: value.showHiddenFiles === true,
    alternateRowColors: value.alternateRowColors !== false,
    restoreSession: value.restoreSession !== false,
    restoreTerminalPanel: value.restoreTerminalPanel === true,
    confirmDelete: value.confirmDelete !== false,
    deleteMode: normalizeDeleteMode(value.deleteMode),
    terminalStartsInActiveFolder: value.terminalStartsInActiveFolder !== false,
    editorCommand: normalizeEditorCommand(value.editorCommand),
    customTools: normalizeCustomTools(value.customTools),
    listColumnWidths: normalizeListColumnWidths(value.listColumnWidths),
  };
}

function remoteVolumeIdFromPath(path) {
  const cleanPath = String(path || '').trim();

  if (!cleanPath.startsWith('remote://')) {
    return '';
  }

  return cleanPath.slice('remote://'.length).split('/')[0] || '';
}

function remoteVolumeIdsFromPaths(paths = []) {
  return [...new Set(
    (Array.isArray(paths) ? paths : [paths])
      .map(remoteVolumeIdFromPath)
      .filter(Boolean),
  )];
}

function remoteHealthColor(health) {
  switch (health?.status) {
    case 'connected':
      return '#34c759';
    case 'idle':
      return '#8E8E93';
    case 'checking':
      return '#5ca8ff';
    case 'authRequired':
      return '#ff9f0a';
    case 'error':
      return '#ff453a';
    default:
      return '#8E8E93';
  }
}

function remoteHealthRefreshInterval(id, health, isActive) {
  if (isActive) {
    return REMOTE_HEALTH_ACTIVE_REFRESH_INTERVAL_MS;
  }

  if (health?.status === 'error' || health?.status === 'authRequired') {
    const failures = Math.max(1, Number(remoteHealthBackoff.get(id)?.failures || 1));
    return Math.min(
      REMOTE_HEALTH_ERROR_MAX_INTERVAL_MS,
      REMOTE_HEALTH_ERROR_BASE_INTERVAL_MS * (2 ** (failures - 1)),
    );
  }

  return REMOTE_HEALTH_IDLE_REFRESH_INTERVAL_MS;
}

function isRemoteHealthStale(id, health, isActive = false) {
  if (!health || health.status === 'unknown') {
    return true;
  }

  if (health.status === 'checking' || health.status === 'idle') {
    return false;
  }

  const checkedAtMs = Number(health.checkedAt || 0) * 1000;
  const backoff = remoteHealthBackoff.get(id);

  if (!isActive && backoff?.nextCheckAt && Date.now() < backoff.nextCheckAt) {
    return false;
  }

  return !checkedAtMs || Date.now() - checkedAtMs >= remoteHealthRefreshInterval(id, health, isActive);
}

function registerRemoteHealthFailure(id) {
  const failures = Math.max(1, Number(remoteHealthBackoff.get(id)?.failures || 0) + 1);
  const interval = Math.min(
    REMOTE_HEALTH_ERROR_MAX_INTERVAL_MS,
    REMOTE_HEALTH_ERROR_BASE_INTERVAL_MS * (2 ** (failures - 1)),
  );
  remoteHealthBackoff.set(id, {
    failures,
    nextCheckAt: Date.now() + interval,
  });
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
  tab.selectedIndex = -1;
  tab.selectionAnchorIndex = -1;
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
    selectedIndex: -1,
    selectionAnchorIndex: -1,
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

function fileNameForPath(path) {
  const cleanPath = String(path || '').replace(/\/+$/, '');

  if (!cleanPath || cleanPath === '/' || cleanPath === '~') {
    return cleanPath || '~';
  }

  return cleanPath.split('/').filter(Boolean).pop() || cleanPath;
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

function isLikelyMissingPathError(error) {
  const code = String(error?.code || '').toLowerCase();
  const message = normalizeError(error);

  return code === 'not_found'
    || code === 'file_not_found'
    || (code === 'io_error' && MISSING_PATH_ERROR_PATTERN.test(message))
    || MISSING_PATH_ERROR_PATTERN.test(message);
}

function isUnlockAuthError(error) {
  const code = String(error?.code || '').toLowerCase();
  const message = normalizeError(error).toLowerCase();

  return code === 'volume_unlock_auth_failed'
    || message.includes('password was not accepted')
    || message.includes('incorrect passphrase');
}

function localVolumeNeedsUnlock(volume) {
  return Boolean(
    volume?.devicePath
    && volume?.isEncrypted
    && volume?.needsUnlock,
  );
}

function localMountCandidatesForPath(path, options = {}) {
  const cleanPath = String(path || '').trim().replace(/\/+$/, '');

  if (
    !cleanPath
    || cleanPath === '~'
    || !cleanPath.startsWith('/')
    || cleanPath.startsWith('remote://')
    || isArchivePath(cleanPath)
  ) {
    return [];
  }

  const parts = cleanPath.split('/').filter(Boolean);
  const homeUserName = String(options.homeUserName || '').trim();
  const candidates = [];
  const seenRoots = new Set();
  const addCandidate = (volumeIndex) => {
    if (volumeIndex < 0 || volumeIndex >= parts.length) {
      return;
    }

    const root = `/${parts.slice(0, volumeIndex + 1).join('/')}`;

    if (seenRoots.has(root)) {
      return;
    }

    seenRoots.add(root);
    candidates.push({
      root,
      volumeName: parts[volumeIndex],
      suffix: parts.slice(volumeIndex + 1).join('/'),
    });
  };

  if (parts[0] === 'run' && parts[1] === 'media' && parts.length >= 4) {
    addCandidate(3);
  }

  if (parts[0] === 'media') {
    const secondSegmentIsCurrentUser = homeUserName && mountNamesMatch(parts[1], homeUserName);

    if (secondSegmentIsCurrentUser && parts.length >= 3) {
      addCandidate(2);
    }

    if (parts.length >= 2) {
      addCandidate(1);
    }

    if (!secondSegmentIsCurrentUser && parts.length >= 3) {
      addCandidate(2);
    }
  }

  if (parts[0] === 'mnt' && parts.length >= 2) {
    addCandidate(1);
  }

  if (parts[0] === 'Volumes' && parts.length >= 2) {
    addCandidate(1);
  }

  return candidates;
}

function safeDecodePathName(name) {
  try {
    return decodeURIComponent(name);
  } catch {
    return name;
  }
}

function mountNameKeys(name) {
  const normalizedName = String(name || '').trim().toLocaleLowerCase();

  if (!normalizedName) {
    return new Set();
  }

  const decodedName = safeDecodePathName(normalizedName);
  const keys = new Set([normalizedName, decodedName]);

  for (const key of [...keys]) {
    keys.add(key.replace(/[_-]+/g, ' '));
    keys.add(key.replace(/\s+/g, '_'));
    keys.add(key.replace(/\s+/g, '-'));
    keys.add(key.replace(/[\s_-]+/g, ''));
  }

  return keys;
}

function mountNamesMatch(left, right) {
  const leftKeys = mountNameKeys(left);
  const rightKeys = mountNameKeys(right);

  if (leftKeys.size === 0 || rightKeys.size === 0) {
    return false;
  }

  return [...leftKeys].some((key) => rightKeys.has(key));
}

function volumeMatchesMountCandidate(volume, candidate) {
  if (!volume || !candidate || String(volume.path || '').startsWith('remote://')) {
    return false;
  }

  const mountedPath = normalizeComparablePath(volume.path || '');
  const candidateRoot = normalizeComparablePath(candidate.root);

  if (mountedPath && mountedPath === candidateRoot) {
    return true;
  }

  return [
    volume.name,
    fileNameForPath(volume.path),
  ].some((name) => mountNamesMatch(name, candidate.volumeName));
}

function findLocalMountRecovery(path, availableVolumes = [], options = {}) {
  const candidates = localMountCandidatesForPath(path, options);

  for (const candidate of candidates) {
    const volume = availableVolumes.find((candidateVolume) => {
      if (!candidateVolume?.devicePath || !volumeMatchesMountCandidate(candidateVolume, candidate)) {
        return false;
      }

      const mountedPath = normalizeComparablePath(candidateVolume.path || '');
      const requestedRoot = normalizeComparablePath(candidate.root);

      return !candidateVolume.isMounted || mountedPath !== requestedRoot;
    });

    if (volume) {
      return { candidate, volume };
    }
  }

  return null;
}

function pathWithMountedRoot(candidate, mountedRoot) {
  const root = String(mountedRoot || '').trim().replace(/\/+$/, '');

  if (!root) {
    return '';
  }

  return candidate.suffix ? `${root}/${candidate.suffix}` : root;
}

function pathIsInsideRoot(path, root) {
  const normalizedPath = normalizeComparablePath(path);
  const normalizedRoot = normalizeComparablePath(root);

  if (!normalizedPath || !normalizedRoot) {
    return false;
  }

  if (normalizedRoot === '/') {
    return normalizedPath.startsWith('/');
  }

  return normalizedPath === normalizedRoot || normalizedPath.startsWith(`${normalizedRoot}/`);
}

function replaceTabCurrentPath(tab, previousPath, nextPath) {
  const normalizedPath = String(nextPath || '').trim();

  if (!tab || !normalizedPath || normalizedPath === previousPath) {
    return;
  }

  tab.currentPath = normalizedPath;

  if (tab.history[tab.historyIndex] === previousPath) {
    tab.history.splice(tab.historyIndex, 1, normalizedPath);
  } else if (tab.historyIndex >= 0 && tab.historyIndex < tab.history.length) {
    tab.history.splice(tab.historyIndex, 1, normalizedPath);
  }
}

function isWatchableLocalDirectoryPath(path) {
  const value = normalizeComparablePath(path);

  return Boolean(
    value
    && !value.startsWith('remote://')
    && !isArchivePath(value)
    && (value.startsWith('/') || /^[a-zA-Z]:[\\/]/.test(value))
  );
}

function resolveHomeRelativePath(path, home) {
  const value = normalizeComparablePath(path);
  const homePath = normalizeComparablePath(home);

  if (!homePath || homePath === '~') {
    return value;
  }

  if (value === '~') {
    return homePath;
  }

  if (value.startsWith('~/')) {
    return `${homePath}/${value.slice(2)}`;
  }

  return value;
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

function normalizeWorkspaceName(name) {
  return String(name || '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, WORKSPACE_NAME_LIMIT);
}

function workspaceIdForName(name, timestamp = Date.now()) {
  const slug = normalizeWorkspaceName(name)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 42);

  return `workspace-${slug || 'saved'}-${Number(timestamp || Date.now()).toString(36)}`;
}

function normalizeWorkspaceHistory(history, path) {
  const normalizedPath = String(path || '~').trim() || '~';
  const entries = (Array.isArray(history) ? history : [])
    .map((entry) => String(entry || '').trim())
    .filter(Boolean)
    .slice(-NAV_HISTORY_LIMIT);

  return entries.length > 0 ? entries : [normalizedPath];
}

function normalizeWorkspaceTab(tab = {}, fallbackPath = '~', fallbackViewMode = 'list') {
  const value = tab && typeof tab === 'object' ? tab : {};
  const path = String(value.path || value.currentPath || fallbackPath || '~').trim() || fallbackPath || '~';
  let history = normalizeWorkspaceHistory(value.history, path);
  let historyIndex = Number.isInteger(value.historyIndex)
    ? Math.min(Math.max(value.historyIndex, 0), history.length - 1)
    : history.length - 1;

  if (history[historyIndex] !== path) {
    const existingIndex = history.lastIndexOf(path);

    if (existingIndex >= 0) {
      historyIndex = existingIndex;
    } else {
      history = [...history.slice(0, historyIndex + 1), path].slice(-NAV_HISTORY_LIMIT);
      historyIndex = history.length - 1;
    }
  }

  return {
    path: history[historyIndex] || path,
    viewMode: normalizeViewMode(value.viewMode || fallbackViewMode),
    sortKey: normalizeSortKey(value.sortKey),
    sortDirection: normalizeSortDirection(value.sortDirection),
    history,
    historyIndex,
  };
}

function normalizeWorkspacePane(pane = {}, fallbackPath = '~', fallbackViewMode = 'list') {
  const value = pane && typeof pane === 'object' ? pane : {};
  const rawTabs = Array.isArray(value.tabs) ? value.tabs : [];
  const tabs = rawTabs
    .slice(0, WORKSPACE_TAB_LIMIT)
    .map((tab) => normalizeWorkspaceTab(tab, fallbackPath, fallbackViewMode));
  const normalizedTabs = tabs.length
    ? tabs
    : [normalizeWorkspaceTab({ path: fallbackPath, viewMode: fallbackViewMode }, fallbackPath, fallbackViewMode)];
  const rawActiveIndex = Number(value.activeIndex);
  const activeIndex = Number.isFinite(rawActiveIndex)
    ? Math.min(Math.max(Math.trunc(rawActiveIndex), 0), normalizedTabs.length - 1)
    : 0;

  return {
    activeIndex,
    tabs: normalizedTabs,
  };
}

function normalizeWorkspace(workspace, index = 0) {
  const value = workspace && typeof workspace === 'object' ? workspace : {};
  const name = normalizeWorkspaceName(value.name) || `Workspace ${index + 1}`;
  const createdAt = Number.isFinite(Number(value.createdAt)) ? Number(value.createdAt) : Date.now();
  const updatedAt = Number.isFinite(Number(value.updatedAt)) ? Number(value.updatedAt) : createdAt;
  const id = String(value.id || workspaceIdForName(name, createdAt)).trim().slice(0, 120)
    || workspaceIdForName(name, createdAt);

  return {
    id,
    name,
    createdAt,
    updatedAt,
    activePaneId: value.activePaneId === 'right' ? 'right' : 'left',
    left: normalizeWorkspacePane(value.left, '~', DEFAULT_APP_SETTINGS.defaultViewMode),
    right: normalizeWorkspacePane(value.right, '~', DEFAULT_APP_SETTINGS.defaultViewMode),
  };
}

function normalizeWorkspaces(workspaces) {
  if (!Array.isArray(workspaces)) {
    return [];
  }

  const seenIds = new Set();

  return workspaces
    .slice(0, WORKSPACE_LIMIT)
    .map((workspace, index) => normalizeWorkspace(workspace, index))
    .map((workspace, index) => {
      let id = workspace.id;

      if (seenIds.has(id)) {
        id = `${id}-${index + 1}`;
      }

      seenIds.add(id);
      return {
        ...workspace,
        id,
      };
    });
}

function cloneSerializedPane(pane) {
  return {
    activeIndex: Number.isInteger(pane?.activeIndex) ? pane.activeIndex : 0,
    tabs: (Array.isArray(pane?.tabs) ? pane.tabs : []).map((tab) => ({
      ...tab,
      history: Array.isArray(tab.history) ? [...tab.history] : [],
    })),
  };
}

function createPaneFromSerializedPane(id, serializedPane, fallbackPath, fallbackViewMode) {
  const normalizedPane = normalizeWorkspacePane(serializedPane, fallbackPath, fallbackViewMode);
  const initialTabs = normalizedPane.tabs.map((tab) => ({
    ...tab,
    history: [...tab.history],
  }));

  initialTabs.activeIndex = normalizedPane.activeIndex;
  return createPane(id, initialTabs, fallbackPath, fallbackViewMode);
}

function historiesEqual(left = [], right = []) {
  if (left.length !== right.length) {
    return false;
  }

  return left.every((path, index) => path === right[index]);
}

function workspaceTabsEqual(left, right) {
  return left?.path === right?.path
    && normalizeViewMode(left?.viewMode) === normalizeViewMode(right?.viewMode)
    && normalizeSortKey(left?.sortKey) === normalizeSortKey(right?.sortKey)
    && normalizeSortDirection(left?.sortDirection) === normalizeSortDirection(right?.sortDirection)
    && left?.historyIndex === right?.historyIndex
    && historiesEqual(left?.history || [], right?.history || []);
}

function workspacePanesEqual(leftPane, rightPane) {
  const left = normalizeWorkspacePane(leftPane);
  const right = normalizeWorkspacePane(rightPane);

  if (left.activeIndex !== right.activeIndex || left.tabs.length !== right.tabs.length) {
    return false;
  }

  return left.tabs.every((tab, index) => workspaceTabsEqual(tab, right.tabs[index]));
}

function workspaceMatchesPaneState(workspace, paneState) {
  return workspacePanesEqual(workspace?.left, paneState?.left)
    && workspacePanesEqual(workspace?.right, paneState?.right);
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
  const dialog = useDialog();
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
  applyAccentColor(appSettings.value.accentColor);

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

  const activePaneId = ref('left');
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
  const workspaces = ref(normalizeWorkspaces(savedSettings.workspaces));
  const activeWorkspaceId = ref(
    workspaces.value.some((workspace) => workspace.id === savedSettings.activeWorkspaceId)
      ? savedSettings.activeWorkspaceId
      : '',
  );
  const settingsVisible = ref(false);
  const fileSearchVisible = ref(false);
  const fileSearchMode = ref('files');
  const searchQuery = ref('');
  const queue = ref([]);
  const operationLog = ref([]);
  const undoStack = ref([]);
  const redoStack = ref([]);
  const volumes = ref([]);
  const favoriteGroups = ref([]);
  const favorites = ref([]);
  const homeDirectory = ref('');
  const columnPreviewEntries = ref({ left: null, right: null });
  const columnSelectionStates = ref({ left: null, right: null });
  const columnTargetDirectories = ref({ left: null, right: null });
  const columnRefreshRequests = ref({ left: null, right: null });
  const columnSelectionResetKeys = ref({ left: 0, right: 0 });
  const dragOperation = ref(null);
  let initializePromise = null;
  let stopOperationProgressListener = null;
  let nextDirectoryRefreshId = 1;
  let nextFileDragId = 1;
  let claimedFileDragDropId = null;

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
    ];

    const knownGroupIds = new Set(favoriteGroups.value.map((group) => group.id));
    const fallbackGroups = favoriteGroups.value.length
      ? favoriteGroups.value
      : [{
        id: DEFAULT_FAVORITE_GROUP_ID,
        name: 'Favorites',
        sortOrder: 0,
      }];
    const groupedFavorites = new Map(
      fallbackGroups.map((group) => [group.id, []]),
    );

    for (const favorite of favorites.value) {
      const groupId = knownGroupIds.has(favorite.groupId)
        ? favorite.groupId
        : DEFAULT_FAVORITE_GROUP_ID;

      if (!groupedFavorites.has(groupId)) {
        groupedFavorites.set(groupId, []);
      }

      groupedFavorites.get(groupId).push({
        ...favorite,
        favoriteGroupId: groupId,
        icon: favorite.icon || 'folder',
        color: favorite.color || '#5ca8ff',
        isFavorite: true,
        matchPrefix: true,
      });
    }

    const volumeItems = volumes.value.map((volume) => {
      const isRemote = volume.path?.startsWith('remote://');
      const isMountable = !volume.isMounted && Boolean(volume.devicePath);
      const needsUnlock = localVolumeNeedsUnlock(volume);

      return {
        name: volume.name,
        path: volume.path,
        devicePath: volume.devicePath,
        detail: isRemote ? '' : volume.detail || 'Mounted',
        icon: isRemote ? 'network' : needsUnlock ? 'lock' : 'drive',
        color: isRemote ? remoteHealthColor(volume.health) : needsUnlock ? '#ff9f0a' : volume.isRemovable ? '#5ca8ff' : '#8E8E93',
        disabled: !volume.isMounted && !isMountable,
        isMountable,
        isRemote,
        isEncrypted: Boolean(volume.isEncrypted),
        needsUnlock,
        canEject: !isRemote && Boolean(volume.devicePath),
        remoteId: isRemote ? remoteVolumeIdFromPath(volume.path) : '',
        remoteHealth: volume.health || null,
        remoteCapabilities: volume.capabilities || null,
        matchPrefix: true,
      };
    });

    const deviceItems = volumeItems.filter((item) => !item.isRemote);
    const remoteItems = volumeItems.filter((item) => item.isRemote);

    sections.splice(
      1,
      0,
      {
        title: 'Devices',
        items: deviceItems,
      },
    );

    const favoriteSections = fallbackGroups.map((group) => ({
      id: group.id,
      title: group.name,
      favoriteGroupId: group.id,
      isFavoriteGroup: true,
      isDefaultFavoriteGroup: group.id === DEFAULT_FAVORITE_GROUP_ID,
      items: groupedFavorites.get(group.id) || [],
    }));

    return [
      ...sections,
      ...favoriteSections,
      ...(remoteItems.length > 0 ? [{
        title: 'Remote Storage',
        items: remoteItems,
      }] : []),
    ];
  });

  const activePane = computed(() => activeTabFor(activePaneId.value));
  const canGoBack = computed(() => canGoBackInTab(activePane.value));
  const canGoForward = computed(() => canGoForwardInTab(activePane.value));
  const canUndo = computed(() => undoStack.value.length > 0);
  const canRedo = computed(() => {
    const next = redoStack.value.at(-1);

    if (!next) {
      return false;
    }

    // Re-deleting is only offered while Trash mode is on, so a redone delete
    // stays reversible.
    if (next.kind === 'delete' && appSettings.value.deleteMode !== 'trash') {
      return false;
    }

    return true;
  });
  const undoLabel = computed(() => undoStack.value.at(-1)?.label || '');
  const redoLabel = computed(() => (canRedo.value ? redoStack.value.at(-1)?.label || '' : ''));
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
  const activeWorkspace = computed(() =>
    workspaces.value.find((workspace) => workspace.id === activeWorkspaceId.value) || null,
  );

  const activeRemoteVolumeIds = computed(() => {
    const ids = new Set();

    Object.values(panes.value).forEach((pane) => {
      (pane?.tabs || []).forEach((tab) => {
        remoteVolumeIdsFromPaths(tab.currentPath).forEach((id) => ids.add(id));
      });
    });

    Object.values(columnTargetDirectories.value || {}).forEach((path) => {
      remoteVolumeIdsFromPaths(path).forEach((id) => ids.add(id));
    });

    queue.value
      .filter((job) => ACTIVE_QUEUE_STATUSES.has(job.status))
      .forEach((job) => {
        remoteVolumeIdsFromPaths([
          ...(Array.isArray(job.remotePaths) ? job.remotePaths : []),
          job.currentPath,
        ]).forEach((id) => ids.add(id));

        (Array.isArray(job.remoteIds) ? job.remoteIds : [])
          .filter(Boolean)
          .forEach((id) => ids.add(id));
      });

    return [...ids].sort();
  });

  function resetPaneTransientState() {
    columnPreviewEntries.value = { left: null, right: null };
    columnSelectionStates.value = { left: null, right: null };
    columnTargetDirectories.value = { left: null, right: null };
    columnRefreshRequests.value = { left: null, right: null };
    columnSelectionResetKeys.value = {
      left: (columnSelectionResetKeys.value.left || 0) + 1,
      right: (columnSelectionResetKeys.value.right || 0) + 1,
    };
  }

  function saveCurrentWorkspace(name, options = {}) {
    const workspaceName = normalizeWorkspaceName(name);

    if (!workspaceName) {
      return null;
    }

    const now = Date.now();
    const existingWorkspace = options.updateExisting === false
      ? null
      : activeWorkspace.value
        || workspaces.value.find((workspace) => workspace.name.toLowerCase() === workspaceName.toLowerCase())
        || null;
    const paneState = persistedPaneState.value;
    const workspace = normalizeWorkspace({
      id: existingWorkspace?.id || workspaceIdForName(workspaceName, now),
      name: workspaceName,
      createdAt: existingWorkspace?.createdAt || now,
      updatedAt: now,
      activePaneId: activePaneId.value,
      left: cloneSerializedPane(paneState.left),
      right: cloneSerializedPane(paneState.right),
    }, workspaces.value.length);

    if (existingWorkspace) {
      workspaces.value = workspaces.value.map((candidate) =>
        candidate.id === existingWorkspace.id ? workspace : candidate,
      );
    } else {
      workspaces.value = [workspace, ...workspaces.value].slice(0, WORKSPACE_LIMIT);
    }

    activeWorkspaceId.value = workspace.id;
    return workspace;
  }

  function updateWorkspaceFromCurrent(workspaceId) {
    const existingWorkspace = workspaces.value.find((workspace) => workspace.id === workspaceId);

    if (!existingWorkspace) {
      return null;
    }

    const now = Date.now();
    const paneState = persistedPaneState.value;
    const workspace = normalizeWorkspace({
      ...existingWorkspace,
      updatedAt: now,
      activePaneId: activePaneId.value,
      left: cloneSerializedPane(paneState.left),
      right: cloneSerializedPane(paneState.right),
    });

    workspaces.value = workspaces.value.map((candidate) =>
      candidate.id === existingWorkspace.id ? workspace : candidate,
    );
    activeWorkspaceId.value = workspace.id;
    return workspace;
  }

  function renameWorkspace(workspaceId, name) {
    const existingWorkspace = workspaces.value.find((workspace) => workspace.id === workspaceId);
    const workspaceName = normalizeWorkspaceName(name);

    if (!existingWorkspace || !workspaceName) {
      return null;
    }

    const workspace = normalizeWorkspace({
      ...existingWorkspace,
      name: workspaceName,
      updatedAt: Date.now(),
    });

    workspaces.value = workspaces.value.map((candidate) =>
      candidate.id === existingWorkspace.id ? workspace : candidate,
    );
    return workspace;
  }

  async function applyWorkspace(workspaceId) {
    const workspace = workspaces.value.find((candidate) => candidate.id === workspaceId);

    if (!workspace) {
      if (activeWorkspaceId.value === workspaceId) {
        activeWorkspaceId.value = '';
      }

      return false;
    }

    const normalizedWorkspace = normalizeWorkspace(workspace);
    workspaces.value = workspaces.value.map((candidate) =>
      candidate.id === workspace.id ? normalizedWorkspace : candidate,
    );
    panes.value = {
      left: createPaneFromSerializedPane(
        'left',
        normalizedWorkspace.left,
        normalizedWorkspace.left.tabs[normalizedWorkspace.left.activeIndex]?.path || '~',
        appSettings.value.defaultViewMode,
      ),
      right: createPaneFromSerializedPane(
        'right',
        normalizedWorkspace.right,
        normalizedWorkspace.right.tabs[normalizedWorkspace.right.activeIndex]?.path || '~',
        appSettings.value.defaultViewMode,
      ),
    };
    resetPaneTransientState();
    activePaneId.value = normalizedWorkspace.activePaneId;
    activeWorkspaceId.value = normalizedWorkspace.id;

    await Promise.all([
      loadPane('left'),
      loadPane('right'),
    ]);

    return true;
  }

  function removeWorkspace(workspaceId) {
    const id = String(workspaceId || '').trim();

    if (!id) {
      return false;
    }

    const nextWorkspaces = workspaces.value.filter((workspace) => workspace.id !== id);

    if (nextWorkspaces.length === workspaces.value.length) {
      return false;
    }

    workspaces.value = nextWorkspaces;

    if (activeWorkspaceId.value === id) {
      activeWorkspaceId.value = '';
    }

    return true;
  }

  async function initialize() {
    if (initializePromise) {
      return initializePromise;
    }

    initializePromise = (async () => {
      await loadAppSettings();
      await loadHomeDirectory();
      await Promise.all([
        initializeOperationProgressListener(),
        initializeRemoteEditSyncListener(),
        initializeDirectoryWatchListener(),
      ]);
      await Promise.all([
        loadFavoriteGroups(),
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
        const home = homeDirectory.value || await getHomeDirectory();
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

  async function loadHomeDirectory() {
    try {
      homeDirectory.value = await getHomeDirectory();
    } catch {
      homeDirectory.value = '';
    }
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

  async function initializeRemoteEditSyncListener() {
    if (stopRemoteEditSyncListener || !canUseLocalFileAssets()) {
      return;
    }

    let stopSynced = null;
    let stopFailed = null;

    try {
      stopSynced = await listen('remote-edit-synced', (event) => {
        const path = event.payload?.path || '';

        if (!path) {
          return;
        }

        clearRemotePreviewCache(path);
        scheduleDirectoryReloadInPanes(parentPathFor(path));
        addOperationLog({
          operation: 'remote-edit',
          label: 'Remote edit synced',
          detail: fileNameForPath(path),
          status: 'completed',
          path,
        });
      });
      stopFailed = await listen('remote-edit-sync-failed', (event) => {
        const path = event.payload?.path || '';
        const message = event.payload?.message || 'Unable to sync remote edit.';

        addOperationLog({
          operation: 'remote-edit',
          label: 'Remote edit not synced',
          detail: message,
          status: 'failed',
          path,
        });
      });

      stopRemoteEditSyncListener = () => {
        stopSynced();
        stopFailed();
      };
    } catch {
      stopSynced?.();
      stopFailed?.();
      stopRemoteEditSyncListener = null;
    }
  }

  async function initializeDirectoryWatchListener() {
    if (stopDirectoryWatchListener || !canUseLocalFileAssets()) {
      return;
    }

    try {
      stopDirectoryWatchListener = await listen('directory-watch-changed', (event) => {
        const path = normalizeComparablePath(event.payload?.path || '');

        if (!path || !activeDirectoryWatchPaths.value.includes(path)) {
          return;
        }

        scheduleDirectoryReloadInPanes(path);
      });

      scheduleActiveDirectoryWatchSync(activeDirectoryWatchPaths.value);
    } catch {
      stopDirectoryWatchListener = null;
    }
  }

  function scheduleActiveDirectoryWatchSync(paths = activeDirectoryWatchPaths.value) {
    if (!canUseLocalFileAssets()) {
      return;
    }

    const normalizedPaths = [...new Set(
      (Array.isArray(paths) ? paths : [])
        .map(normalizeComparablePath)
        .filter(isWatchableLocalDirectoryPath),
    )].sort();
    const key = normalizedPaths.join('\0');

    if (key === lastWatchedDirectoryPaths) {
      return;
    }

    lastWatchedDirectoryPaths = key;

    if (activeDirectoryWatchSyncTimer) {
      globalThis.clearTimeout(activeDirectoryWatchSyncTimer);
    }

    activeDirectoryWatchSyncTimer = globalThis.setTimeout(async () => {
      activeDirectoryWatchSyncTimer = null;

      try {
        const watchedPaths = await watchActiveDirectories(normalizedPaths);
        lastWatchedDirectoryPaths = [...new Set(
          (Array.isArray(watchedPaths) ? watchedPaths : [])
            .map(normalizeComparablePath)
            .filter(isWatchableLocalDirectoryPath),
        )].sort().join('\0');
      } catch {
        lastWatchedDirectoryPaths = '';
      }
    }, 120);
  }

  async function refreshVolumes() {
    try {
      const previous = new Map(volumes.value.map((volume) => [volume.path, volume]));
      volumes.value = (await listVolumes()).map((volume) => {
        const previousVolume = previous.get(volume.path);

        if (
          volume.path?.startsWith('remote://') &&
          previousVolume?.health &&
          (!volume.health || volume.health.status === 'unknown')
        ) {
          return {
            ...volume,
            health: previousVolume.health,
          };
        }

        return volume;
      });
      refreshRemoteHealth();
    } catch {
      volumes.value = [];
    }
  }

  function volumeForDevicePath(devicePath) {
    const normalizedDevicePath = String(devicePath || '').trim();

    if (!normalizedDevicePath) {
      return null;
    }

    return volumes.value.find((volume) =>
      volume.devicePath === normalizedDevicePath
      && !String(volume.path || '').startsWith('remote://'),
    ) || null;
  }

  async function promptEncryptedVolumePassword(volume, lastError = null) {
    const name = volume?.name || 'Encrypted Device';

    return dialog.prompt({
      title: lastError ? 'Password Not Accepted' : 'Unlock Device',
      message: `Enter the password for ${name}.`,
      detail: volume?.devicePath || '',
      inputLabel: 'Device password',
      inputType: 'password',
      inputRequired: true,
      confirmLabel: 'Unlock',
      icon: 'lock',
      variant: lastError ? 'warning' : 'default',
    });
  }

  async function unlockLocalVolumeWithPrompt(volume) {
    const devicePath = String(volume?.devicePath || '').trim();

    if (!devicePath) {
      throw new Error('No device path is available for this encrypted volume.');
    }

    if (activeLocalVolumeUnlocks.has(devicePath)) {
      return activeLocalVolumeUnlocks.get(devicePath);
    }

    const unlockPromise = (async () => {
      let lastError = null;

      while (true) {
        const password = await promptEncryptedVolumePassword(volume, lastError);

        if (password === null) {
          return null;
        }

        try {
          return await unlockVolume(devicePath, password);
        } catch (error) {
          if (isUnlockAuthError(error)) {
            lastError = error;
            continue;
          }

          throw error;
        }
      }
    })();

    activeLocalVolumeUnlocks.set(devicePath, unlockPromise);

    try {
      return await unlockPromise;
    } finally {
      activeLocalVolumeUnlocks.delete(devicePath);
    }
  }

  async function mountLocalVolume(volume) {
    const devicePath = String(volume?.devicePath || '').trim();

    if (!devicePath) {
      return null;
    }

    const currentVolume = volumeForDevicePath(devicePath) || volume;

    if (localVolumeNeedsUnlock(currentVolume)) {
      return unlockLocalVolumeWithPrompt(currentVolume);
    }

    try {
      return await mountVolume(devicePath);
    } catch (error) {
      await refreshVolumes();

      const refreshedVolume = volumeForDevicePath(devicePath) || currentVolume;

      if (localVolumeNeedsUnlock(refreshedVolume)) {
        return unlockLocalVolumeWithPrompt(refreshedVolume);
      }

      throw error;
    }
  }

  function clearTabForEjectedRoot(tab, rootPath) {
    if (!tab || !rootPath) {
      return false;
    }

    const wasInsideRoot = pathIsInsideRoot(tab.currentPath, rootPath)
      || pathIsInsideRoot(tab.entriesPath, rootPath);

    if (!wasInsideRoot) {
      return false;
    }

    replaceTabCurrentPath(tab, tab.currentPath, '~');
    tab.entries = [];
    tab.entriesPath = '';
    tab.loaded = false;
    tab.selectedIndex = -1;
    tab.selectionAnchorIndex = -1;
    tab.selectedPaths = [];
    tab.error = '';
    return true;
  }

  async function ejectLocalVolume(volume) {
    const devicePath = String(volume?.devicePath || '').trim();

    if (!devicePath) {
      return false;
    }

    const currentVolume = volumeForDevicePath(devicePath) || volume;
    const mountedRoot = normalizeComparablePath(currentVolume?.path || volume?.path || '');
    const mountedRootsBeforeEject = volumes.value
      .filter((candidate) => candidate?.path && !candidate.path.startsWith('remote://'))
      .map((candidate) => normalizeComparablePath(candidate.path))
      .filter(Boolean);

    let ejectError = null;
    let ejectResult = null;

    try {
      ejectResult = await ejectVolume(devicePath);
    } catch (error) {
      ejectError = error;
    } finally {
      await refreshVolumes();
    }

    const removedRoots = mountedRootsBeforeEject.filter((root) => (
      !volumes.value.some((candidate) => normalizeComparablePath(candidate.path) === root)
    ));
    const affectedRoots = [
      mountedRoot,
      ...(Array.isArray(ejectResult?.mountPaths) ? ejectResult.mountPaths : []),
      ...removedRoots,
    ]
      .map(normalizeComparablePath)
      .filter(Boolean)
      .filter((root, index, roots) => roots.indexOf(root) === index);

    if (affectedRoots.length > 0) {
      const rootsMissingAfterEject = affectedRoots.filter((root) => (
        !volumes.value.some((candidate) => normalizeComparablePath(candidate.path) === root)
      ));
      const rootsToClear = ejectError ? rootsMissingAfterEject : affectedRoots;

      if (rootsToClear.length > 0) {
        const reloads = [];

        for (const paneId of ['left', 'right']) {
          const pane = panes.value[paneId];

          for (const tab of pane.tabs) {
            const changed = rootsToClear.some((root) => clearTabForEjectedRoot(tab, root));

            if (changed && pane.activeTabId === tab.id) {
              reloads.push(loadPane(paneId, tab.id));
            }
          }
        }

        await Promise.allSettled(reloads);
      }
    }

    if (ejectError) {
      throw ejectError;
    }

    addOperationLog({
      operation: 'eject',
      label: `Ejected ${currentVolume?.name || volume?.name || 'device'}`,
      detail: devicePath,
      status: 'completed',
      path: devicePath,
    });

    return true;
  }

  async function autoMountMissingLocalVolume(path, error) {
    if (!isLikelyMissingPathError(error) || localMountCandidatesForPath(path).length === 0) {
      return null;
    }

    const home = homeDirectory.value || await getHomeDirectory().catch(() => '');
    const candidateOptions = { homeUserName: home ? fileNameForPath(home) : '' };

    let recovery = findLocalMountRecovery(path, volumes.value, candidateOptions);

    if (!recovery) {
      await refreshVolumes();
      recovery = findLocalMountRecovery(path, volumes.value, candidateOptions);
    }

    if (!recovery) {
      return null;
    }

    const { candidate, volume } = recovery;
    let mountedByCarelo = false;

    try {
      let mountedVolume = volume;

      if (!mountedVolume.isMounted || !mountedVolume.path) {
        mountedVolume = await mountLocalVolume(volume);
        mountedByCarelo = true;
      }

      if (!mountedVolume) {
        return {
          error: {
            message: `Unlock cancelled for ${volume.name || candidate.volumeName}.`,
          },
        };
      }

      await refreshVolumes();

      const refreshedVolume = volumeForDevicePath(mountedVolume?.devicePath || volume.devicePath);
      const resolvedVolume = refreshedVolume || mountedVolume || volume;
      const resolvedPath = pathWithMountedRoot(candidate, resolvedVolume?.path);

      if (!resolvedPath) {
        return {
          error: {
            message: `Unable to mount ${volume.name || candidate.volumeName} automatically. Carelo could not resolve its mount point.`,
          },
        };
      }

      return {
        path: resolvedPath,
        volume: resolvedVolume,
        mounted: mountedByCarelo,
      };
    } catch (mountError) {
      await refreshVolumes();

      return {
        error: {
          message: `Unable to mount ${volume.name || candidate.volumeName} automatically. ${normalizeError(mountError)}`,
        },
      };
    }
  }

  async function refreshRemoteHealth(remoteId = '') {
    if (remoteHealthRefreshInFlight && !remoteId) {
      return;
    }

    const activeRemoteIds = new Set(activeRemoteVolumeIds.value);
    const remotes = volumes.value
      .filter((volume) => volume.path?.startsWith('remote://'))
      .filter((volume) => {
        const id = remoteVolumeIdFromPath(volume.path);

        if (remoteId) {
          return id === remoteId;
        }

        return isRemoteHealthStale(id, volume.health, activeRemoteIds.has(id));
      });

    if (remotes.length === 0) {
      return;
    }

    if (!remoteId) {
      remoteHealthRefreshInFlight = true;
    }

    const markChecking = (id) => {
      volumes.value = volumes.value.map((volume) => (
        remoteVolumeIdFromPath(volume.path) === id
          ? {
            ...volume,
            health: {
              status: 'checking',
              message: null,
              checkedAt: null,
            },
          }
          : volume
      ));
    };

    const markHealthError = (id, error) => {
      registerRemoteHealthFailure(id);

      volumes.value = volumes.value.map((volume) => (
        remoteVolumeIdFromPath(volume.path) === id
          ? {
            ...volume,
            health: {
              status: 'error',
              message: error?.message || 'Unable to check remote connection.',
              checkedAt: Math.floor(Date.now() / 1000),
            },
          }
          : volume
      ));
    };

    try {
      await Promise.allSettled(remotes.map(async (volume) => {
        const id = remoteVolumeIdFromPath(volume.path);

        if (!id) {
          return;
        }

        markChecking(id);
        let remote;

        try {
          remote = await checkRemoteVolume(id);
        } catch (error) {
          markHealthError(id, error);
          return;
        }

        const nextHealth = remote.health || null;

        if (nextHealth?.status === 'error' || nextHealth?.status === 'authRequired') {
          registerRemoteHealthFailure(id);
        } else {
          remoteHealthBackoff.delete(id);
        }

        volumes.value = volumes.value.map((candidate) => (
          remoteVolumeIdFromPath(candidate.path) === id
            ? {
              ...candidate,
              capabilities: remote.capabilities || candidate.capabilities,
              health: nextHealth || candidate.health,
            }
            : candidate
        ));
      }));
    } finally {
      if (!remoteId) {
        remoteHealthRefreshInFlight = false;
      }
    }
  }

  function applyRemoteLifecycleState(activeIds = [], released = []) {
    const activeSet = new Set(activeIds);
    const releasedById = new Map(
      (Array.isArray(released) ? released : [])
        .filter((item) => item?.id)
        .map((item) => [item.id, item]),
    );

    if (activeSet.size === 0 && releasedById.size === 0) {
      return;
    }

    const checkedAt = Math.floor(Date.now() / 1000);

    volumes.value = volumes.value.map((volume) => {
      const id = remoteVolumeIdFromPath(volume.path);

      if (!id) {
        return volume;
      }

      if (activeSet.has(id) && volume.health?.status === 'idle') {
        const health = {
          status: 'connected',
          message: null,
          checkedAt,
        };

        return {
          ...volume,
          health,
        };
      }

      if (releasedById.has(id)) {
        const result = releasedById.get(id);
        const status = result.message ? 'connected' : 'idle';
        const health = {
          status,
          message: result.message || 'No open tabs are using this remote volume.',
          checkedAt,
        };

        return {
          ...volume,
          health,
        };
      }

      return volume;
    });
  }

  function scheduleActiveRemoteVolumeSync(ids = activeRemoteVolumeIds.value) {
    const normalizedIds = [...new Set(ids)].filter(Boolean).sort();
    const key = normalizedIds.join('\0');

    applyRemoteLifecycleState(normalizedIds);

    if (key === lastReportedRemoteVolumeIds) {
      return;
    }

    lastReportedRemoteVolumeIds = key;

    if (activeRemoteSyncTimer) {
      globalThis.clearTimeout(activeRemoteSyncTimer);
    }

    activeRemoteSyncTimer = globalThis.setTimeout(async () => {
      activeRemoteSyncTimer = null;

      try {
        const released = await setActiveRemoteVolumes(normalizedIds);
        applyRemoteLifecycleState(normalizedIds, released);
      } catch {
        lastReportedRemoteVolumeIds = '';
      }
    }, 80);
  }

  async function loadFavorites() {
    try {
      favorites.value = await listStoredFavorites();
    } catch {
      favorites.value = [];
    }
  }

  async function loadFavoriteGroups() {
    try {
      favoriteGroups.value = await listStoredFavoriteGroups();
    } catch {
      favoriteGroups.value = [{
        id: DEFAULT_FAVORITE_GROUP_ID,
        name: 'Favorites',
        sortOrder: 0,
      }];
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

  function directoryReloadComparablePath(path) {
    return normalizeComparablePath(resolveHomeRelativePath(path, homeDirectory.value));
  }

  const activeDirectoryWatchPaths = computed(() => {
    const paths = ['left', 'right']
      .map((paneId) => effectiveDirectoryFor(paneId) || activeTabFor(paneId)?.currentPath || '')
      .map(directoryReloadComparablePath)
      .filter(isWatchableLocalDirectoryPath);

    return [...new Set(paths)].sort();
  });

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

    tab.selectedIndex = selectedIndex >= 0 ? selectedIndex : -1;
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
      tab.selectedIndex = -1;
      tab.selectionAnchorIndex = -1;
      tab.selectedPaths = [];
    }

    const loadVersion = tab.loadVersion + 1;
    tab.loadVersion = loadVersion;
    const applyLoadedEntries = (entries, loadedPath) => {
      replaceTabCurrentPath(tab, requestedPath, loadedPath);
      tab.entries = entries;
      tab.entriesPath = loadedPath;
      tab.loaded = true;
      tab.selectedPaths = tab.selectedPaths.filter((path) =>
        entries.some((entry) => entry.path === path),
      );
      const visibleEntries = visibleEntriesForTab(tab, query, showHiddenFiles.value);
      const focusedIndex = focusedPath
        ? visibleEntries.findIndex((entry) => entry.path === focusedPath)
        : -1;
      tab.selectedIndex = focusedIndex >= 0 ? focusedIndex : -1;
      tab.selectionAnchorIndex = tab.selectedIndex;
      scheduleActiveDirectoryWatchSync(activeDirectoryWatchPaths.value);
    };

    try {
      const entries = await listDirectory(requestedPath);

      if (tab.loadVersion !== loadVersion) {
        return;
      }

      applyLoadedEntries(entries, requestedPath);
    } catch (error) {
      if (tab.loadVersion !== loadVersion) {
        return;
      }

      let loadError = error;
      const mountRecovery = await autoMountMissingLocalVolume(requestedPath, error);

      if (tab.loadVersion !== loadVersion) {
        return;
      }

      if (mountRecovery?.path) {
        try {
          const entries = await listDirectory(mountRecovery.path);

          if (tab.loadVersion !== loadVersion) {
            return;
          }

          applyLoadedEntries(entries, mountRecovery.path);

          if (mountRecovery.mounted) {
            addOperationLog({
              operation: 'mount',
              label: `Mounted ${mountRecovery.volume?.name || tabTitleForPath(mountRecovery.path)}`,
              detail: mountRecovery.path,
              status: 'completed',
              path: mountRecovery.path,
            });
          }

          return;
        } catch (retryError) {
          if (tab.loadVersion !== loadVersion) {
            return;
          }

          loadError = retryError;
        }
      } else if (mountRecovery?.error) {
        loadError = mountRecovery.error;
      }

      tab.entries = [];
      tab.entriesPath = requestedPath;
      tab.selectedIndex = -1;
      tab.selectionAnchorIndex = -1;
      tab.loaded = true;
      tab.error = normalizeError(loadError);
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
    const normalizedPath = directoryReloadComparablePath(path);
    const targetPaneIds = Array.isArray(paneIds) && paneIds.length > 0
      ? [...new Set(paneIds.filter((paneId) => panes.value[paneId]))]
      : Object.keys(panes.value);
    const reloads = [];

    for (const paneId of targetPaneIds) {
      const pane = panes.value[paneId];
      requestColumnDirectoryRefresh(paneId, path);

      for (const tab of pane.tabs) {
        if (directoryReloadComparablePath(tab.currentPath) === normalizedPath) {
          reloads.push(loadPane(paneId, tab.id));
        }
      }
    }

    await Promise.all(reloads);
  }

  function scheduleDirectoryReloadInPanes(path) {
    const normalizedPath = directoryReloadComparablePath(path);

    if (!normalizedPath) {
      return;
    }

    scheduledDirectoryReloadPaths.add(normalizedPath);

    if (scheduledDirectoryReloadTimer) {
      return;
    }

    scheduledDirectoryReloadTimer = globalThis.setTimeout(async () => {
      const paths = [...scheduledDirectoryReloadPaths];
      scheduledDirectoryReloadPaths = new Set();
      scheduledDirectoryReloadTimer = null;

      await Promise.all(paths.map((reloadPath) => reloadDirectoryInPanes(reloadPath).catch(() => {})));
    }, DIRECTORY_RELOAD_BATCH_DELAY_MS);
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
      remoteIds: remoteVolumeIdsFromPaths([
        ...(Array.isArray(options.remotePaths) ? options.remotePaths : []),
        ...(Array.isArray(options.remoteIds) ? options.remoteIds.map((id) => `remote://${id}/`) : []),
      ]),
      remotePaths: Array.isArray(options.remotePaths)
        ? options.remotePaths.filter((path) => remoteVolumeIdFromPath(path))
        : [],
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

  // Undo/redo history. Operations record a reversible descriptor here on
  // success; undo/redo replay it with existing transfer commands. Recording a
  // new operation clears the redo stack, matching standard editor semantics.
  function recordHistory(entry = {}) {
    if (!entry || !entry.kind) {
      return;
    }

    const record = {
      id: `hist-${Date.now()}-${nextHistoryId++}`,
      kind: entry.kind,
      label: entry.label || 'operation',
      items: Array.isArray(entry.items) ? entry.items.map((item) => ({ ...item })) : [],
      createdPaths: Array.isArray(entry.createdPaths) ? [...entry.createdPaths] : [],
      paths: Array.isArray(entry.paths) ? [...entry.paths] : [],
      from: entry.from || '',
      to: entry.to || '',
      directories: Array.isArray(entry.directories)
        ? [...new Set(entry.directories.filter(Boolean))]
        : [],
    };

    undoStack.value = [...undoStack.value, record].slice(-HISTORY_LIMIT);
    redoStack.value = [];
  }

  // Only Trash deletes are reversible (undo restores from Trash); permanent
  // deletes are never recorded.
  function recordTrashDelete({ paths = [], directories = [], label = 'Deleted items' } = {}) {
    // Only local Trash deletes are restorable — remote deletes bypass the
    // Trash and archives are read-only.
    const targets = (Array.isArray(paths) ? paths : []).filter(
      (path) => path && !String(path).startsWith('remote://') && !isArchivePath(path),
    );

    if (appSettings.value.deleteMode !== 'trash' || targets.length === 0) {
      return;
    }

    recordHistory({ kind: 'delete', label, paths: targets, directories });
  }

  function clearHistory() {
    undoStack.value = [];
    redoStack.value = [];
  }

  // Runs a single undo/redo step as a queue job so it shows progress, reuses
  // the cancel/pause plumbing, and reports failures like any other transfer.
  // Returns true when applied, false when the user cancelled; throws on error.
  async function runHistoryJob({ operation, label, directories = [], controllable = true, run }) {
    const jobId = startQueueJob({
      operation,
      label,
      remotePaths: directories,
      // Only operations that forward the job id to the backend (move/copy) can
      // honour cancel/pause; rename and trash-delete run atomically.
      cancelable: controllable,
      pausable: controllable,
    });

    try {
      await run(jobId);
      await Promise.all(
        [...new Set(directories.filter(Boolean))].map((path) =>
          reloadDirectoryInPanes(path).catch(() => {}),
        ),
      );
      completeQueueJob(jobId, 'Done');
      return true;
    } catch (error) {
      if (error?.code === 'operation_cancelled') {
        cancelQueueJobDone(jobId);
        return false;
      }

      failQueueJob(jobId, error?.message || `${label} failed.`);
      throw error;
    }
  }

  async function applyHistoryEntry(entry, direction) {
    const undoing = direction === 'undo';
    const verb = undoing ? 'Undo' : 'Redo';

    if (entry.kind === 'move') {
      const items = entry.items.map((item) => ({
        from: undoing ? item.to : item.from,
        to: undoing ? item.from : item.to,
        overwrite: false,
        symlinkMode: item.symlinkMode || 'preserve',
      }));

      return runHistoryJob({
        operation: 'move',
        label: `${verb}: ${entry.label}`,
        directories: entry.directories,
        run: async (jobId) => {
          await moveItems(items, jobId);
          // Keep color tags attached as files move back/forward.
          await moveFileTags(items.map((item) => ({ from: item.from, to: item.to }))).catch(() => {});
        },
      });
    }

    if (entry.kind === 'rename') {
      const from = undoing ? entry.to : entry.from;
      const to = undoing ? entry.from : entry.to;

      return runHistoryJob({
        operation: 'rename',
        label: `${verb}: ${entry.label}`,
        directories: entry.directories,
        controllable: false,
        run: async () => {
          await renameItem(from, to);
          await moveFileTags([{ from, to }]).catch(() => {});
        },
      });
    }

    if (entry.kind === 'copy') {
      if (undoing) {
        return runHistoryJob({
          operation: 'delete',
          label: `${verb}: ${entry.label}`,
          directories: entry.directories,
          controllable: false,
          run: () => deleteItems(entry.createdPaths, 'trash'),
        });
      }

      return runHistoryJob({
        operation: 'copy',
        label: `${verb}: ${entry.label}`,
        directories: entry.directories,
        run: (jobId) => copyItems(entry.items.map((item) => ({ ...item })), jobId),
      });
    }

    if (entry.kind === 'delete') {
      if (undoing) {
        return runHistoryJob({
          operation: 'restore',
          label: `${verb}: ${entry.label}`,
          directories: entry.directories,
          controllable: false,
          run: () => restoreFromTrash(entry.paths),
        });
      }

      // Redo re-deletes to Trash so it stays reversible; only reachable when
      // Trash mode is enabled (see canRedo).
      return runHistoryJob({
        operation: 'delete',
        label: `${verb}: ${entry.label}`,
        directories: entry.directories,
        controllable: false,
        run: () => deleteItems(entry.paths, 'trash'),
      });
    }

    return false;
  }

  async function undoLastOperation() {
    if (historyBusy || undoStack.value.length === 0) {
      return;
    }

    const entry = undoStack.value.at(-1);
    historyBusy = true;
    undoStack.value = undoStack.value.slice(0, -1);

    try {
      const applied = await applyHistoryEntry(entry, 'undo');

      if (applied) {
        redoStack.value = [...redoStack.value, entry].slice(-HISTORY_LIMIT);
      } else {
        undoStack.value = [...undoStack.value, entry];
      }
    } catch {
      // The queue job surfaces the failure; restore the entry so it can retry.
      undoStack.value = [...undoStack.value, entry];
    } finally {
      historyBusy = false;
    }
  }

  async function redoLastOperation() {
    if (historyBusy || !canRedo.value) {
      return;
    }

    const entry = redoStack.value.at(-1);
    historyBusy = true;
    redoStack.value = redoStack.value.slice(0, -1);

    try {
      const applied = await applyHistoryEntry(entry, 'redo');

      if (applied) {
        undoStack.value = [...undoStack.value, entry].slice(-HISTORY_LIMIT);
      } else {
        redoStack.value = [...redoStack.value, entry];
      }
    } catch {
      redoStack.value = [...redoStack.value, entry];
    } finally {
      historyBusy = false;
    }
  }

  function setActivePane(paneId) {
    if (!panes.value[paneId]) {
      return;
    }

    if (activePaneId.value !== paneId) {
      clearInactivePaneSelections(paneId);
    }

    activePaneId.value = paneId;
  }

  function switchActivePane() {
    setActivePane(activePaneId.value === 'left' ? 'right' : 'left');
  }

  function resetPaneSelection(paneId) {
    const tab = activeTabFor(paneId);

    if (!tab) {
      return;
    }

    const columnTarget = columnTargetDirectories.value[paneId];
    const hasNestedColumnPath = tab.viewMode === 'columns'
      && columnTarget
      && normalizeComparablePath(columnTarget) !== normalizeComparablePath(tab.currentPath);

    if (!hasNestedColumnPath) {
      tab.selectedIndex = -1;
      tab.selectionAnchorIndex = -1;
    } else if (tab.selectedIndex >= 0) {
      tab.selectionAnchorIndex = tab.selectedIndex;
    }

    tab.selectedPaths = [];
    clearColumnPreviewEntry(paneId);
    clearColumnSelectionState(paneId);
    columnSelectionResetKeys.value = {
      ...columnSelectionResetKeys.value,
      [paneId]: (columnSelectionResetKeys.value[paneId] || 0) + 1,
    };
  }

  function clearInactivePaneSelections(activePane) {
    Object.keys(panes.value).forEach((paneId) => {
      if (paneId !== activePane) {
        resetPaneSelection(paneId);
      }
    });
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
      clearColumnPreviewEntry(paneId);
      clearColumnSelectionState(paneId);
      clearColumnTargetDirectory(paneId);
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

    const nextIndex = tab.selectedIndex < 0
      ? delta < 0 ? entries.length - 1 : 0
      : Math.min(entries.length - 1, Math.max(0, tab.selectedIndex + delta));
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

    const id = nextFileDragId;
    nextFileDragId += 1;
    claimedFileDragDropId = null;
    dragOperation.value = {
      id,
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

  function claimFileDrop(id = null) {
    if (!id) {
      return true;
    }

    if (claimedFileDragDropId === id) {
      return false;
    }

    claimedFileDragDropId = id;
    return true;
  }

  function isFavoritePath(path) {
    const normalizedPath = normalizeComparablePath(path);

    return favorites.value.some((favorite) =>
      normalizeComparablePath(favorite.path) === normalizedPath,
    );
  }

  function favoriteInputForEntry(entry, groupId = DEFAULT_FAVORITE_GROUP_ID) {
    return {
      groupId,
      name: entry.name,
      path: entry.path,
      icon: entry.path === '~' ? 'home' : 'folder',
      color: '#5ca8ff',
    };
  }

  async function addFavoritesFromEntries(
    entries,
    targetIndex = null,
    groupId = DEFAULT_FAVORITE_GROUP_ID,
  ) {
    const directories = (entries || []).filter((entry) =>
      entry?.kind === 'directory' && entry.path && !isArchivePath(entry.path),
    );

    if (directories.length === 0) {
      return [];
    }

    const added = [];
    let insertIndex = Number.isInteger(targetIndex)
      ? targetIndex
      : favorites.value.filter((favorite) =>
        (favorite.groupId || DEFAULT_FAVORITE_GROUP_ID) === groupId,
      ).length;

    for (const entry of directories) {
      const favorite = await addStoredFavorite(favoriteInputForEntry(entry, groupId));
      added.push(favorite);
      favorites.value = await moveStoredFavorite(favorite.id, insertIndex, groupId);
      insertIndex += 1;
    }

    return added;
  }

  async function addFavoriteGroup(name) {
    const group = await addStoredFavoriteGroup(name);
    const existingIndex = favoriteGroups.value.findIndex((item) => item.id === group.id);

    if (existingIndex >= 0) {
      favoriteGroups.value.splice(existingIndex, 1, group);
    } else {
      favoriteGroups.value.push(group);
      favoriteGroups.value.sort((a, b) =>
        (a.sortOrder ?? 0) - (b.sortOrder ?? 0)
        || NAME_COLLATOR.compare(a.name || '', b.name || ''),
      );
    }

    return group;
  }

  async function removeFavoriteGroup(id) {
    if (!id || id === DEFAULT_FAVORITE_GROUP_ID) {
      return;
    }

    await removeStoredFavoriteGroup(id);
    await Promise.all([
      loadFavoriteGroups(),
      loadFavorites(),
    ]);
  }

  async function removeFavorite(id) {
    if (!id) {
      return;
    }

    await removeStoredFavorite(id);
    favorites.value = favorites.value.filter((favorite) => favorite.id !== id);
  }

  async function moveFavorite(id, targetIndex, targetGroupId = null) {
    if (!id) {
      return;
    }

    favorites.value = await moveStoredFavorite(id, targetIndex, targetGroupId);
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
    fileSearchMode.value = ['commands', 'content', 'files'].includes(mode) ? mode : 'files';
    fileSearchVisible.value = true;
  }

  function openCommandPalette() {
    openFileSearch('commands');
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
      workspaces.value,
      activeWorkspaceId.value,
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
        workspaces: workspaces.value,
        activeWorkspaceId: activeWorkspaceId.value,
      });
    },
    { deep: true },
  );

  watch(
    () => persistedPaneState.value,
    (paneState) => {
      const workspace = activeWorkspace.value;

      if (workspace && !workspaceMatchesPaneState(workspace, paneState)) {
        activeWorkspaceId.value = '';
      }
    },
    { deep: true, immediate: true },
  );

  watch(
    appSettings,
    (settings) => {
      const normalizedSettings = normalizeAppSettings(settings);
      applyAppearanceMode(normalizedSettings.appearanceMode);
      applyColorScheme(normalizedSettings.colorScheme);
      applyAccentColor(normalizedSettings.accentColor);
      saveUiSettings({ appSettings: normalizedSettings });
      saveStoredAppSettings(normalizedSettings).catch(() => {});
    },
    { deep: true },
  );

  watch(
    activeRemoteVolumeIds,
    (ids) => {
      scheduleActiveRemoteVolumeSync(ids);
    },
    { immediate: true },
  );

  watch(
    activeDirectoryWatchPaths,
    (paths) => {
      scheduleActiveDirectoryWatchSync(paths);
    },
    { immediate: true },
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
    workspaces,
    activeWorkspaceId,
    activeWorkspace,
    settingsVisible,
    fileSearchVisible,
    fileSearchMode,
    appSettings,
    searchQuery,
    queue,
    operationLog,
    undoStack,
    redoStack,
    canUndo,
    canRedo,
    undoLabel,
    redoLabel,
    volumes,
    favoriteGroups,
    favorites,
    columnRefreshRequests,
    columnSelectionResetKeys,
    dragOperation,
    sidebarSections,
    initialize,
    refreshVolumes,
    mountLocalVolume,
    ejectLocalVolume,
    refreshRemoteHealth,
    activeTabFor,
    parentDirectoryFor,
    effectiveDirectoryFor,
    tabTitle,
    visibleEntriesFor,
    selectedEntryFor,
    saveCurrentWorkspace,
    updateWorkspaceFromCurrent,
    renameWorkspace,
    applyWorkspace,
    removeWorkspace,
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
    recordHistory,
    recordTrashDelete,
    clearHistory,
    undoLastOperation,
    redoLastOperation,
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
    claimFileDrop,
    isFavoritePath,
    addFavoriteGroup,
    removeFavoriteGroup,
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
    openCommandPalette,
    openFileSearch,
    openContentSearch,
    closeFileSearch,
    toggleFileSearch,
  };
});

// Hot-swap the store on edit instead of leaving a stale instance (which made
// newly added actions appear `undefined` until a full reload).
if (import.meta.hot) {
  import.meta.hot.accept(acceptHMRUpdate(useFileManagerStore, import.meta.hot));
}
