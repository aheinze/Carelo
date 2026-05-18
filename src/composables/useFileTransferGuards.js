import {
  areSameVolume,
  copyItems,
  listDirectory,
  moveItems,
  renameItem,
} from './useFileOperations';
import { useDialog } from './useDialog';
import { useFileManagerStore } from '../stores/fileManagerStore';

export function joinPath(directory, name) {
  if (!directory || directory === '/') {
    return `/${name}`;
  }

  return directory.endsWith('/') ? `${directory}${name}` : `${directory}/${name}`;
}

export function cleanPath(path) {
  return String(path || '').replace(/\/+$/, '') || '/';
}

export function parentPath(path) {
  const clean = cleanPath(path);

  if (!clean || clean === '/' || clean === '~') {
    return clean || '~';
  }

  const index = clean.lastIndexOf('/');
  return index <= 0 ? '/' : clean.slice(0, index);
}

export function siblingPath(path, nextName) {
  return joinPath(parentPath(path), nextName);
}

export function isSameOrChildPath(path, parent) {
  const child = cleanPath(path);
  const base = cleanPath(parent);

  return child === base || (base !== '/' && child.startsWith(`${base}/`));
}

function transferItemContainsPath(item, path) {
  const candidate = cleanPath(path);

  if (!candidate) {
    return false;
  }

  return [item?.from, item?.to].some((root) => root && isSameOrChildPath(candidate, root));
}

function retryItemsForFailure(items, failedPath) {
  const fallback = items.map((item) => ({ ...item }));

  if (!failedPath) {
    return fallback;
  }

  const failedIndex = items.findIndex((item) => transferItemContainsPath(item, failedPath));

  if (failedIndex < 0) {
    return fallback;
  }

  return items.slice(failedIndex).map((item) => ({ ...item }));
}

function itemLabel(count) {
  return count === 1 ? '1 item' : `${count} items`;
}

function entryKindLabel(entry) {
  return entry?.kind === 'directory' ? 'folder' : 'file';
}

function namesPreview(entries) {
  const names = entries.slice(0, 3).map((entry) => `"${entry.name}"`);
  const extra = entries.length > 3 ? ` and ${entries.length - 3} more` : '';
  return `${names.join(', ')}${extra}`;
}

async function entriesByName(directory) {
  const entries = await listDirectory(directory);
  return new Map(entries.map((entry) => [entry.name, entry]));
}

function targetNameFor(entry, nameForEntry) {
  if (typeof nameForEntry !== 'function') {
    return entry.name;
  }

  return nameForEntry(entry) || entry.name;
}

function isDirectoryEntry(entry) {
  return entry?.kind === 'directory';
}

function isReservedEntry(entry) {
  return Boolean(entry?.__careloReserved);
}

function formatSize(size) {
  const bytes = Number(size);

  if (!Number.isFinite(bytes)) {
    return '';
  }

  if (bytes >= 1024 ** 3) {
    return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  }

  if (bytes >= 1024 ** 2) {
    return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  }

  if (bytes >= 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }

  return `${bytes} B`;
}

function formatModified(modifiedAt) {
  if (!modifiedAt) {
    return '';
  }

  return new Intl.DateTimeFormat(undefined, {
    dateStyle: 'short',
    timeStyle: 'short',
  }).format(new Date(modifiedAt * 1000));
}

function entrySummary(entry) {
  const details = [entryKindLabel(entry)];
  const size = formatSize(entry?.size);
  const modified = formatModified(entry?.modifiedAt);

  if (size) {
    details.push(size);
  }

  if (modified) {
    details.push(`modified ${modified}`);
  }

  return details.join(', ');
}

function splitCopyName(name, entry) {
  const value = String(name || '').trim() || entry?.name || 'Untitled';

  if (isDirectoryEntry(entry)) {
    return { stem: value, extension: '' };
  }

  const dotIndex = value.lastIndexOf('.');

  if (dotIndex <= 0) {
    return { stem: value, extension: '' };
  }

  return {
    stem: value.slice(0, dotIndex),
    extension: value.slice(dotIndex),
  };
}

function copyNameFor(name, entry, index) {
  const { stem, extension } = splitCopyName(name, entry);
  const suffix = index === 1 ? ' copy' : ` copy ${index}`;

  return `${stem}${suffix}${extension}`;
}

function uniqueTargetName(name, targetEntries, entry) {
  if (!targetEntries.has(name)) {
    return name;
  }

  for (let index = 1; index < 10000; index += 1) {
    const candidate = copyNameFor(name, entry, index);

    if (!targetEntries.has(candidate)) {
      return candidate;
    }
  }

  return copyNameFor(name, entry, Date.now());
}

function reserveTarget(targetEntries, name, path, entry) {
  targetEntries.set(name, {
    name,
    path,
    kind: entry?.kind || 'file',
    __careloReserved: true,
  });
}

function shouldUseFolderConflictActions(entry, existingEntry) {
  return isDirectoryEntry(entry) || isDirectoryEntry(existingEntry);
}

export function forcedTransferModeFromEvent(event) {
  if (!event) {
    return null;
  }

  if (event.ctrlKey || event.altKey) {
    return 'copy';
  }

  if (event.shiftKey) {
    return 'move';
  }

  return null;
}

export function dropEffectFromEvent(event, fallback = 'move') {
  return forcedTransferModeFromEvent(event) || fallback;
}

export function useFileTransferGuards() {
  const dialog = useDialog();
  const store = useFileManagerStore();

  async function chooseConflictResolution({
    entry,
    existingEntry,
    targetName,
    conflictKind,
    allowApplyToAll,
  }) {
    const folderConflict = conflictKind === 'folder';
    const result = await dialog.choice({
      title: folderConflict ? 'Folder Name Conflict' : 'File Already Exists',
      message: `"${targetName}" already exists in the destination.`,
      detail: folderConflict
        ? 'Choose a unique name for the incoming item or skip it. Replacing folders is intentionally blocked.'
        : 'Replacing will overwrite the existing file. Keep Both creates a unique name for the incoming file.',
      variant: 'warning',
      icon: folderConflict ? 'folder' : 'file',
      facts: [
        { label: 'Incoming', value: entrySummary(entry) },
        { label: 'Existing', value: entrySummary(existingEntry) },
      ],
      checkboxLabel: allowApplyToAll ? `Apply to all ${conflictKind} conflicts` : '',
      actions: folderConflict
        ? [
            { value: 'cancel', label: 'Cancel', cancel: true },
            { value: 'skip', label: 'Skip' },
            { value: 'keepBoth', label: 'Keep Both', primary: true },
          ]
        : [
            { value: 'cancel', label: 'Cancel', cancel: true },
            { value: 'skip', label: 'Skip' },
            { value: 'keepBoth', label: 'Keep Both' },
            { value: 'replace', label: 'Replace', primary: true },
          ],
    });

    if (!result) {
      return null;
    }

    return {
      action: result.value,
      applyToAll: Boolean(result.applyToAll),
    };
  }

  async function chooseSymlinkMode(entries, mode, requestedMode = null) {
    if (requestedMode === 'follow' || requestedMode === 'preserve') {
      return requestedMode;
    }

    const hasSelectedSymlink = entries.some((entry) => entry?.isSymlink || entry?.kind === 'symlink');

    if (!hasSelectedSymlink) {
      return 'preserve';
    }

    const result = await dialog.choice({
      title: mode === 'move' ? 'Move Symbolic Links' : 'Copy Symbolic Links',
      message: 'The selection contains symbolic links.',
      detail: 'Preserving links keeps the link itself. Resolving targets copies the linked file or folder contents.',
      icon: 'file',
      actions: [
        { value: 'cancel', label: 'Cancel', cancel: true },
        { value: 'follow', label: 'Resolve Targets' },
        { value: 'preserve', label: 'Preserve Links', primary: true, default: true },
      ],
    });

    return result?.value || null;
  }

  async function prepareTransfer({
    entries,
    mode,
    targetDirectory,
    nameForEntry = null,
    symlinkMode = null,
  }) {
    const sourceEntries = (Array.isArray(entries) ? entries : []).filter(Boolean);

    if (sourceEntries.length === 0 || !targetDirectory) {
      return null;
    }

    const targetEntries = await entriesByName(targetDirectory);
    const invalid = [];
    const skipped = [];
    const items = [];
    const resolvedSymlinkMode = await chooseSymlinkMode(sourceEntries, mode, symlinkMode);
    const conflictPolicies = {
      file: null,
      folder: null,
    };

    if (!resolvedSymlinkMode) {
      return null;
    }

    for (const entry of sourceEntries) {
      let targetName = targetNameFor(entry, nameForEntry);
      let targetPath = joinPath(targetDirectory, targetName);
      let overwrite = false;

      if (entry.kind === 'directory' && isSameOrChildPath(targetDirectory, entry.path)) {
        invalid.push({ entry, reason: 'contained-target' });
        continue;
      }

      if (cleanPath(targetPath) === cleanPath(entry.path)) {
        invalid.push({ entry, reason: 'same-location' });
        continue;
      }

      const existingEntry = targetEntries.get(targetName);

      if (existingEntry && cleanPath(existingEntry.path) !== cleanPath(entry.path)) {
        if (isReservedEntry(existingEntry)) {
          targetName = uniqueTargetName(targetName, targetEntries, entry);
          targetPath = joinPath(targetDirectory, targetName);
        } else {
          const conflictKind = shouldUseFolderConflictActions(entry, existingEntry)
            ? 'folder'
            : 'file';
          let resolution = conflictPolicies[conflictKind];

          if (!resolution) {
            resolution = await chooseConflictResolution({
              entry,
              existingEntry,
              targetName,
              conflictKind,
              allowApplyToAll: sourceEntries.length > 1,
            });

            if (!resolution) {
              return null;
            }

            if (resolution.applyToAll) {
              conflictPolicies[conflictKind] = resolution;
            }
          }

          if (resolution.action === 'skip') {
            skipped.push({ entry, reason: 'conflict' });
            continue;
          }

          if (resolution.action === 'keepBoth') {
            targetName = uniqueTargetName(targetName, targetEntries, entry);
            targetPath = joinPath(targetDirectory, targetName);
          } else if (resolution.action === 'replace') {
            overwrite = true;
          }
        }
      }

      items.push({
        from: entry.path,
        to: targetPath,
        overwrite,
        symlinkMode: resolvedSymlinkMode,
      });
      reserveTarget(targetEntries, targetName, targetPath, entry);
    }

    if (invalid.length === sourceEntries.length) {
      await dialog.alert({
        title: mode === 'move' ? 'Move Not Possible' : 'Copy Not Possible',
        message: `No ${entryKindLabel(sourceEntries[0])} can be ${mode === 'move' ? 'moved' : 'copied'} there.`,
        detail: 'The target is the same location or inside one of the selected folders.',
        variant: 'warning',
      });
      return null;
    }

    if (items.length === 0) {
      return null;
    }

    if (invalid.length > 0) {
      await dialog.alert({
        title: 'Some Items Skipped',
        message: `${itemLabel(invalid.length)} could not be ${mode === 'move' ? 'moved' : 'copied'}.`,
        detail: 'Skipped items would have ended up in the same location or inside themselves.',
        variant: 'warning',
      });
    }

    if (skipped.length > 0 && skipped.length !== sourceEntries.length) {
      await dialog.alert({
        title: 'Some Items Skipped',
        message: `${itemLabel(skipped.length)} were skipped because of name conflicts.`,
        detail: namesPreview(skipped.map((item) => item.entry)),
        variant: 'warning',
      });
    }

    return items;
  }

  async function copyEntries(options) {
    const transferOptions = { ...options, mode: 'copy' };
    const items = await prepareTransfer(transferOptions);

    if (!items) {
      return false;
    }

    await runQueuedTransfer({
      ...transferOptions,
      items,
    });
    return true;
  }

  async function moveEntries(options) {
    const transferOptions = { ...options, mode: 'move' };
    const items = await prepareTransfer(transferOptions);

    if (!items) {
      return false;
    }

    await runQueuedTransfer({
      ...transferOptions,
      items,
    });
    return true;
  }

  async function defaultTransferMode(entries, targetDirectory) {
    const paths = (Array.isArray(entries) ? entries : [])
      .map((entry) => entry?.path)
      .filter(Boolean);

    if (paths.length === 0 || !targetDirectory) {
      return 'copy';
    }

    try {
      return await areSameVolume(paths, targetDirectory) ? 'move' : 'copy';
    } catch {
      return 'copy';
    }
  }

  async function transferModeForEvent(event, entries, targetDirectory) {
    return forcedTransferModeFromEvent(event) || defaultTransferMode(entries, targetDirectory);
  }

  async function transferEntries(options = {}) {
    return options.mode === 'move'
      ? moveEntries(options)
      : copyEntries({ ...options, mode: 'copy' });
  }

  async function runQueuedTransfer({ items, entries, mode, targetDirectory }) {
    const sourceEntries = (Array.isArray(entries) ? entries : []).filter(Boolean);
    const operationLabel = mode === 'move' ? 'Moving' : 'Copying';
    const itemText = itemLabel(items.length);
    const sourceParents = sourceEntries.map((entry) => parentPath(entry.path));
    const touchedDirectories = mode === 'move'
      ? [targetDirectory, ...sourceParents]
      : [targetDirectory];
    const runItems = items.map((item) => ({ ...item }));
    let retryItems = runItems.map((item) => ({ ...item }));
    const retryAction = () => runQueuedTransfer({
      items: retryItems,
      entries: sourceEntries,
      mode,
      targetDirectory,
    });
    const jobId = store.startQueueJob({
      operation: mode,
      label: `${operationLabel} ${itemText}`,
      detail: targetDirectory ? `To ${targetDirectory}` : '',
      retryAction,
    });

    try {
      if (mode === 'move') {
        await moveItems(runItems, jobId);
      } else {
        await copyItems(runItems, jobId);
      }

      await Promise.all(
        [...new Set(touchedDirectories.filter(Boolean))]
          .map((path) => store.reloadDirectoryInPanes(path)),
      );
      store.completeQueueJob(jobId, `${itemText} ${mode === 'move' ? 'moved' : 'copied'}`);
    } catch (error) {
      if (error?.code === 'operation_cancelled') {
        store.cancelQueueJobDone(jobId);
        return;
      }

      const currentJob = store.queue.find((job) => job.id === jobId);
      retryItems = retryItemsForFailure(runItems, error?.path || currentJob?.currentPath);
      store.failQueueJob(jobId, error?.message || `${operationLabel} failed.`, {
        failedItems: retryItems.map((item) => ({
          path: item.from,
          message: error?.message || 'Failed',
        })),
      });
      throw error;
    }
  }

  async function renameEntry(entry, nextName) {
    const targetName = String(nextName || '').trim();

    if (!entry || !targetName || targetName === entry.name) {
      return false;
    }

    const targetDirectory = parentPath(entry.path);
    const targetPath = joinPath(targetDirectory, targetName);

    if (cleanPath(targetPath) === cleanPath(entry.path)) {
      return false;
    }

    const targetEntries = await entriesByName(targetDirectory);
    const existingEntry = targetEntries.get(targetName);
    let resolvedTargetPath = targetPath;

    if (existingEntry && cleanPath(existingEntry.path) !== cleanPath(entry.path)) {
      const folderConflict = shouldUseFolderConflictActions(entry, existingEntry);
      const result = await dialog.choice({
        title: folderConflict ? 'Rename Conflict' : 'Replace Existing File?',
        message: `"${targetName}" already exists in this folder.`,
        detail: folderConflict
          ? 'Replacing folders during rename is intentionally blocked.'
          : 'Replacing will overwrite the existing file.',
        variant: 'warning',
        icon: folderConflict ? 'folder' : 'file',
        facts: [
          { label: 'Renaming', value: entrySummary(entry) },
          { label: 'Existing', value: entrySummary(existingEntry) },
        ],
        actions: folderConflict
          ? [
              { value: 'cancel', label: 'Cancel', cancel: true },
              { value: 'keepBoth', label: 'Keep Both', primary: true },
            ]
          : [
              { value: 'cancel', label: 'Cancel', cancel: true },
              { value: 'keepBoth', label: 'Keep Both' },
              { value: 'replace', label: 'Replace', primary: true },
            ],
      });

      if (!result) {
        return false;
      }

      if (result.value === 'keepBoth') {
        const resolvedTargetName = uniqueTargetName(targetName, targetEntries, entry);
        resolvedTargetPath = joinPath(targetDirectory, resolvedTargetName);
      }
    }

    await renameItem(entry.path, resolvedTargetPath);
    return true;
  }

  return {
    copyEntries,
    defaultTransferMode,
    moveEntries,
    renameEntry,
    transferEntries,
    transferModeForEvent,
  };
}
