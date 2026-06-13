import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { useDialog } from './useDialog';
import { isArchivePath } from '../utils/archivePaths';

function hasTauriBridge() {
  return (
    typeof window !== 'undefined' &&
    window.__TAURI_INTERNALS__ &&
    typeof window.__TAURI_INTERNALS__.invoke === 'function'
  );
}

function bridgeUnavailableError() {
  return {
    code: 'tauri_bridge_unavailable',
    message: 'Local file access is available only in the Tauri desktop app. Start the app with npm run tauri dev, not npm run dev.',
  };
}

const REMOTE_PREVIEW_CACHE_TTL_MS = 60_000;
const REMOTE_PREVIEW_CACHE_MAX_ENTRIES = 32;
const REMOTE_MEDIA_PREVIEW_CACHE_MAX_BYTES = 8 * 1024 * 1024;
const DELETE_MODES = new Set(['trash', 'permanent']);
const remotePreviewCache = new Map();

function remotePreviewCacheKey(kind, path, maxBytes) {
  return `${kind}\0${maxBytes}\0${path}`;
}

function cachedRemotePreview(kind, path, maxBytes) {
  const key = remotePreviewCacheKey(kind, path, maxBytes);
  const entry = remotePreviewCache.get(key);

  if (!entry) {
    return null;
  }

  if (entry.expiresAt <= Date.now()) {
    remotePreviewCache.delete(key);
    return null;
  }

  remotePreviewCache.delete(key);
  remotePreviewCache.set(key, entry);
  return entry.value;
}

function cacheRemotePreview(kind, path, maxBytes, value, byteLength = 0) {
  if (byteLength > REMOTE_MEDIA_PREVIEW_CACHE_MAX_BYTES) {
    return;
  }

  const key = remotePreviewCacheKey(kind, path, maxBytes);
  remotePreviewCache.set(key, {
    value,
    expiresAt: Date.now() + REMOTE_PREVIEW_CACHE_TTL_MS,
  });

  while (remotePreviewCache.size > REMOTE_PREVIEW_CACHE_MAX_ENTRIES) {
    const oldestKey = remotePreviewCache.keys().next().value;
    remotePreviewCache.delete(oldestKey);
  }
}

export function clearRemotePreviewCache(pathPrefix = '') {
  const prefix = String(pathPrefix || '');

  if (!prefix) {
    remotePreviewCache.clear();
    return;
  }

  for (const key of remotePreviewCache.keys()) {
    if (key.endsWith(`\0${prefix}`)) {
      remotePreviewCache.delete(key);
    }
  }
}

function previewPayloadByteLength(payload) {
  if (!payload) {
    return 0;
  }

  if (payload instanceof ArrayBuffer) {
    return payload.byteLength;
  }

  if (ArrayBuffer.isView(payload)) {
    return payload.byteLength;
  }

  if (Array.isArray(payload)) {
    return payload.length;
  }

  if (typeof payload.text === 'string') {
    return new TextEncoder().encode(payload.text).byteLength;
  }

  return 0;
}

export function canUseLocalFileAssets() {
  return hasTauriBridge();
}

export function isRemotePath(path) {
  return String(path || '').startsWith('remote://');
}

export function localFileAssetUrl(path) {
  if (!hasTauriBridge() || !path || isRemotePath(path) || isArchivePath(path)) {
    return '';
  }

  return convertFileSrc(path);
}

const sudoActions = {
  list_directory: 'list this folder',
  get_file_metadata: 'read this item',
  create_folder: 'create this folder',
  rename_item: 'rename this item',
  delete_items: 'delete the selected items',
  copy_items: 'copy the selected items',
  move_items: 'move the selected items',
  archive_items: 'create this archive',
  unarchive_items: 'extract this archive',
};

function isPermissionError(error) {
  const code = String(error?.code || '');
  const message = String(error?.message || error || '');

  return code === 'permission_denied' || /permission denied/i.test(message);
}

function isSudoRetryError(error) {
  const code = String(error?.code || '');

  return code === 'sudo_auth_failed' || code === 'sudo_password_required';
}

function includesRemotePath(value) {
  if (!value) {
    return false;
  }

  if (typeof value === 'string') {
    return isRemotePath(value);
  }

  if (Array.isArray(value)) {
    return value.some((item) => includesRemotePath(item));
  }

  if (typeof value === 'object') {
    return Object.values(value).some((item) => includesRemotePath(item));
  }

  return false;
}

function operationPath(args = {}, error = null) {
  return error?.path || args.path || args.from || args.to || args.paths?.[0] || args.items?.[0]?.from || '';
}

function normalizeDeleteMode(mode) {
  return DELETE_MODES.has(mode) ? mode : 'trash';
}

async function promptSudoPassword(command, args, error, retry = false) {
  const dialog = useDialog();
  const action = sudoActions[command] || 'complete this operation';
  const path = operationPath(args, error);

  return dialog.prompt({
    title: retry ? 'Sudo Authentication Failed' : 'Administrator Privileges Required',
    message: retry
      ? 'The sudo password was not accepted. Enter it again to retry.'
      : `Carelo needs administrator privileges to ${action}.`,
    detail: path ? `Path: ${path}` : '',
    inputLabel: 'Sudo password',
    inputType: 'password',
    inputRequired: true,
    confirmLabel: 'Authenticate',
    cancelLabel: 'Cancel',
    variant: retry ? 'warning' : 'default',
  });
}

async function retryCommandWithSudo(command, args, originalError) {
  let lastError = originalError;

  for (let attempt = 0; attempt < 2; attempt += 1) {
    const password = await promptSudoPassword(command, args, lastError, attempt > 0);

    if (password === null) {
      throw lastError;
    }

    try {
      return await invoke(command, {
        ...args,
        sudoPassword: password,
      });
    } catch (error) {
      lastError = error;

      if (!isSudoRetryError(error)) {
        throw error;
      }
    }
  }

  throw lastError;
}

async function invokeCommand(command, args = {}, options = {}) {
  if (!hasTauriBridge()) {
    throw bridgeUnavailableError();
  }

  try {
    return await invoke(command, args);
  } catch (error) {
    if (
      options.sudo !== true ||
      args.sudoPassword ||
      includesRemotePath(args) ||
      !isPermissionError(error)
    ) {
      throw error;
    }

    return retryCommandWithSudo(command, args, error);
  }
}

export async function listDirectory(path) {
  return invokeCommand('list_directory', { path }, { sudo: true });
}

export async function searchFiles(root, query, options = {}, jobId = null) {
  return invokeCommand('search_files', { root, query, options, jobId });
}

export async function searchContent(root, query, options = {}, jobId = null) {
  return invokeCommand('search_content', { root, query, options, jobId });
}

export async function getFileMetadata(path) {
  return invokeCommand('get_file_metadata', { path }, { sudo: true });
}

export async function getGitFileInfo(path) {
  return invokeCommand('get_git_file_info', { path });
}

export async function compareFileChecksums(leftPath, rightPath) {
  return invokeCommand('compare_file_checksums', { leftPath, rightPath });
}

export async function computeFileChecksum(path) {
  return invokeCommand('compute_file_checksum', { path });
}

export async function compareDirectories(left, right, options = {}) {
  return invokeCommand('compare_directories', { left, right, options });
}

export async function readTextPreview(path, maxBytes = 96 * 1024) {
  if (isRemotePath(path)) {
    const cached = cachedRemotePreview('text', path, maxBytes);

    if (cached) {
      return cached;
    }

    const preview = await invokeCommand('read_text_preview', { path, maxBytes });
    cacheRemotePreview('text', path, maxBytes, preview, previewPayloadByteLength(preview));
    return preview;
  }

  return invokeCommand('read_text_preview', { path, maxBytes });
}

export async function readMediaPreview(path, maxBytes = 128 * 1024 * 1024) {
  if (isRemotePath(path)) {
    const cached = cachedRemotePreview('media', path, maxBytes);

    if (cached) {
      return cached;
    }

    const preview = await invokeCommand('read_media_preview', { path, maxBytes });
    cacheRemotePreview('media', path, maxBytes, preview, previewPayloadByteLength(preview));
    return preview;
  }

  return invokeCommand('read_media_preview', { path, maxBytes });
}

export async function createMediaStreamUrl(path) {
  if (!hasTauriBridge() || !path || isArchivePath(path)) {
    return '';
  }

  return invokeCommand('create_media_stream_url', { path });
}

export async function getHomeDirectory() {
  return invokeCommand('get_home_directory');
}

export async function listVolumes() {
  return invokeCommand('list_volumes');
}

export async function watchActiveDirectories(paths) {
  return invokeCommand('watch_active_directories', { paths });
}

export async function mountVolume(devicePath) {
  return invokeCommand('mount_volume', { devicePath });
}

export async function unlockVolume(devicePath, password) {
  return invokeCommand('unlock_volume', { devicePath, password });
}

export async function ejectVolume(devicePath) {
  return invokeCommand('eject_volume', { devicePath });
}

export async function areSameVolume(paths, targetDirectory) {
  return invokeCommand('same_volume', { paths, targetDirectory });
}

export async function writeSystemFileClipboard(mode, paths) {
  return invokeCommand('write_system_file_clipboard', { payload: { mode, paths } });
}

export async function readSystemFileClipboard() {
  return invokeCommand('read_system_file_clipboard');
}

export async function addRemoteVolume(config) {
  return invokeCommand('add_remote_volume', { config });
}

export async function removeRemoteVolume(id) {
  return invokeCommand('remove_remote_volume', { id });
}

export async function listRemoteVolumes() {
  return invokeCommand('list_remote_volumes');
}

export async function checkRemoteVolume(id) {
  return invokeCommand('check_remote_volume', { id });
}

export async function setActiveRemoteVolumes(ids) {
  return invokeCommand('set_active_remote_volumes', { ids });
}

export async function listFavorites() {
  return invokeCommand('list_favorites');
}

export async function listFavoriteGroups() {
  return invokeCommand('list_favorite_groups');
}

export async function addFavoriteGroup(name) {
  return invokeCommand('add_favorite_group', { name });
}

export async function removeFavoriteGroup(id) {
  return invokeCommand('remove_favorite_group', { id });
}

export async function addFavorite(favorite) {
  return invokeCommand('add_favorite', { favorite });
}

export async function removeFavorite(id) {
  return invokeCommand('remove_favorite', { id });
}

export async function moveFavorite(id, targetIndex, targetGroupId = null) {
  return invokeCommand('move_favorite', { id, targetIndex, targetGroupId });
}

export async function appStorePath() {
  return invokeCommand('app_store_path');
}

export async function getAppSettings() {
  return invokeCommand('get_app_settings');
}

export async function saveAppSettings(settings) {
  return invokeCommand('save_app_settings', { settings });
}

export async function createOAuthTokens(provider, clientId, clientSecret = '') {
  return invokeCommand('create_oauth_tokens', {
    provider,
    clientId,
    clientSecret: clientSecret || null,
  });
}

export async function createFolder(path) {
  return invokeCommand('create_folder', { path }, { sudo: true });
}

export async function renameItem(from, to) {
  return invokeCommand('rename_item', { from, to }, { sudo: true });
}

export async function deleteItems(paths, deleteMode = 'trash') {
  const mode = normalizeDeleteMode(deleteMode);

  return invokeCommand('delete_items', { paths, deleteMode: mode }, { sudo: mode === 'permanent' });
}

export async function restoreFromTrash(paths) {
  return invokeCommand('restore_from_trash', { paths });
}

export async function copyItems(items, jobId = null) {
  return invokeCommand('copy_items', { items, jobId }, { sudo: true });
}

export async function moveItems(items, jobId = null) {
  return invokeCommand('move_items', { items, jobId }, { sudo: true });
}

export async function archiveItems(paths, destination, options = {}, overwrite = false, jobId = null) {
  return invokeCommand(
    'archive_items',
    { paths, destination, options, overwrite, jobId },
    { sudo: true },
  );
}

export async function unarchiveItems(paths, destinationDirectory, jobId = null) {
  return invokeCommand('unarchive_items', { paths, destinationDirectory, jobId }, { sudo: true });
}

export async function convertImages(paths, options = {}, jobId = null) {
  return invokeCommand('convert_images', { paths, options, jobId });
}

export async function compressPdfs(paths, options = {}, jobId = null) {
  return invokeCommand('compress_pdfs', { paths, options, jobId });
}

export async function runPdfTool(paths, options = {}, jobId = null) {
  return invokeCommand('run_pdf_tool', { paths, options, jobId });
}

export async function measureItemsSize(paths, jobId = null) {
  return invokeCommand('measure_items_size', { paths, jobId });
}

export async function cancelFileOperation(jobId) {
  return invokeCommand('cancel_file_operation', { jobId });
}

export async function pauseFileOperation(jobId) {
  return invokeCommand('pause_file_operation', { jobId });
}

export async function resumeFileOperation(jobId) {
  return invokeCommand('resume_file_operation', { jobId });
}

export async function openWithDefaultApp(path) {
  return invokeCommand('open_with_default_app', { path });
}

export async function listOpenWithApps(path) {
  return invokeCommand('list_open_with_apps', { path });
}

export async function editFile(path, editorCommand = '') {
  return invokeCommand('edit_file', { path, editorCommand: editorCommand || null });
}

export async function openWithApp(path, appId, remember = false) {
  return invokeCommand('open_with_app', { path, appId, remember });
}

export async function runCustomTool(command, paths, cwd = '') {
  return invokeCommand('run_custom_tool', { command, paths, cwd: cwd || null });
}

export async function revealInFileManager(path) {
  return invokeCommand('reveal_in_file_manager', { path });
}

export async function startTerminalSession(cwd) {
  return invokeCommand('terminal_start', { cwd });
}

export async function writeTerminalSession(sessionId, data) {
  return invokeCommand('terminal_write', { sessionId, data });
}

export async function resizeTerminalSession(sessionId, rows, cols) {
  return invokeCommand('terminal_resize', { sessionId, rows, cols });
}

export async function closeTerminalSession(sessionId) {
  return invokeCommand('terminal_close', { sessionId });
}

export async function terminalSessionCwd(sessionId) {
  return invokeCommand('terminal_cwd', { sessionId });
}

export function useFileOperations() {
  return {
    canUseLocalFileAssets,
    archiveItems,
    areSameVolume,
    cancelFileOperation,
    createOAuthTokens,
    createMediaStreamUrl,
    compareFileChecksums,
    computeFileChecksum,
    compareDirectories,
    copyItems,
    createFolder,
    convertImages,
    compressPdfs,
    runPdfTool,
    deleteItems,
    editFile,
    getFileMetadata,
    getGitFileInfo,
    getHomeDirectory,
    addFavorite,
    addFavoriteGroup,
    addRemoteVolume,
    appStorePath,
    checkRemoteVolume,
    isRemotePath,
    listFavoriteGroups,
    listFavorites,
    listDirectory,
    searchContent,
    searchFiles,
    listOpenWithApps,
    listRemoteVolumes,
    listVolumes,
    localFileAssetUrl,
    measureItemsSize,
    mountVolume,
    unlockVolume,
    ejectVolume,
    watchActiveDirectories,
    moveFavorite,
    moveItems,
    openWithApp,
    openWithDefaultApp,
    pauseFileOperation,
    runCustomTool,
    removeFavorite,
    removeFavoriteGroup,
    removeRemoteVolume,
    renameItem,
    readTextPreview,
    readSystemFileClipboard,
    resumeFileOperation,
    revealInFileManager,
    unarchiveItems,
    writeSystemFileClipboard,
    startTerminalSession,
    writeTerminalSession,
    resizeTerminalSession,
    closeTerminalSession,
  };
}
