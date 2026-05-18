import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { useDialog } from './useDialog';

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

export function canUseLocalFileAssets() {
  return hasTauriBridge();
}

export function isRemotePath(path) {
  return String(path || '').startsWith('remote://');
}

export function localFileAssetUrl(path) {
  if (!hasTauriBridge() || !path || isRemotePath(path)) {
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
  unarchive_items: 'extract this zip archive',
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

export async function getFileMetadata(path) {
  return invokeCommand('get_file_metadata', { path }, { sudo: true });
}

export async function getHomeDirectory() {
  return invokeCommand('get_home_directory');
}

export async function listVolumes() {
  return invokeCommand('list_volumes');
}

export async function areSameVolume(paths, targetDirectory) {
  return invokeCommand('same_volume', { paths, targetDirectory });
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

export async function listFavorites() {
  return invokeCommand('list_favorites');
}

export async function addFavorite(favorite) {
  return invokeCommand('add_favorite', { favorite });
}

export async function removeFavorite(id) {
  return invokeCommand('remove_favorite', { id });
}

export async function moveFavorite(id, targetIndex) {
  return invokeCommand('move_favorite', { id, targetIndex });
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

export async function deleteItems(paths) {
  return invokeCommand('delete_items', { paths }, { sudo: true });
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

export async function openWithApp(path, appId, remember = false) {
  return invokeCommand('open_with_app', { path, appId, remember });
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

export function useFileOperations() {
  return {
    canUseLocalFileAssets,
    archiveItems,
    areSameVolume,
    cancelFileOperation,
    createOAuthTokens,
    copyItems,
    createFolder,
    deleteItems,
    getFileMetadata,
    getHomeDirectory,
    addFavorite,
    addRemoteVolume,
    appStorePath,
    isRemotePath,
    listFavorites,
    listDirectory,
    listOpenWithApps,
    listRemoteVolumes,
    listVolumes,
    localFileAssetUrl,
    moveFavorite,
    moveItems,
    openWithApp,
    openWithDefaultApp,
    pauseFileOperation,
    removeFavorite,
    removeRemoteVolume,
    renameItem,
    resumeFileOperation,
    revealInFileManager,
    unarchiveItems,
    startTerminalSession,
    writeTerminalSession,
    resizeTerminalSession,
    closeTerminalSession,
  };
}
