import {
  areSameVolume,
  compareFileChecksums,
  copyItems,
  isRemotePath,
  listDirectory,
  moveItems,
  renameItem,
} from './useFileOperations';
import { useDialog } from './useDialog';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { archiveParentPath, isArchivePath, joinArchiveAwarePath } from '../utils/archivePaths';
import { formatFileDateTime } from '../utils/dateFormat';

export function joinPath(directory, name) {
  if (isArchivePath(directory)) {
    return joinArchiveAwarePath(directory, name);
  }

  if (!directory || directory === '/') {
    return `/${name}`;
  }

  return directory.endsWith('/') ? `${directory}${name}` : `${directory}/${name}`;
}

export function cleanPath(path) {
  const value = String(path || '');

  if (isArchivePath(value)) {
    return value.endsWith('!/') ? value : value.replace(/\/+$/, '');
  }

  return value.replace(/\/+$/, '') || '/';
}

export function parentPath(path) {
  if (isArchivePath(path)) {
    return archiveParentPath(path);
  }

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

function formatModified(modifiedAt, dateFormat = 'system') {
  if (!modifiedAt) {
    return '';
  }

  return formatFileDateTime(modifiedAt, dateFormat, { fallback: '' });
}

function entrySummary(entry, dateFormat = 'system') {
  const details = [entryKindLabel(entry)];
  const size = formatSize(entry?.size);
  const modified = formatModified(entry?.modifiedAt, dateFormat);

  if (size) {
    details.push(size);
  }

  if (modified) {
    details.push(`modified ${modified}`);
  }

  return details.join(', ');
}

function compareSize(entry, existingEntry) {
  const incomingSize = Number(entry?.size);
  const existingSize = Number(existingEntry?.size);

  if (!Number.isFinite(incomingSize) || !Number.isFinite(existingSize)) {
    return 'Unavailable';
  }

  if (incomingSize === existingSize) {
    return `Same size (${formatSize(incomingSize)})`;
  }

  const difference = formatSize(Math.abs(incomingSize - existingSize));
  return incomingSize > existingSize
    ? `Incoming larger by ${difference}`
    : `Existing larger by ${difference}`;
}

function modifiedComparison(entry, existingEntry) {
  const incomingModified = Number(entry?.modifiedAt);
  const existingModified = Number(existingEntry?.modifiedAt);

  if (!Number.isFinite(incomingModified) || !Number.isFinite(existingModified)) {
    return null;
  }

  if (incomingModified === existingModified) {
    return 0;
  }

  return incomingModified > existingModified ? 1 : -1;
}

function compareModified(entry, existingEntry, dateFormat = 'system') {
  const comparison = modifiedComparison(entry, existingEntry);

  if (comparison === null) {
    return 'Unavailable';
  }

  const incoming = formatModified(entry?.modifiedAt, dateFormat) || 'unknown';
  const existing = formatModified(existingEntry?.modifiedAt, dateFormat) || 'unknown';

  if (comparison === 0) {
    return `Same modified time (${incoming})`;
  }

  return comparison > 0
    ? `Incoming newer (${incoming} vs ${existing})`
    : `Existing newer (${existing} vs ${incoming})`;
}

function compareKind(entry, existingEntry) {
  const incomingKind = entryKindLabel(entry);
  const existingKind = entryKindLabel(existingEntry);

  return incomingKind === existingKind
    ? `Same type (${incomingKind})`
    : `Incoming ${incomingKind}, existing ${existingKind}`;
}

function itemName(entry) {
  return entry?.name || entry?.path || 'Untitled';
}

function itemPath(entry) {
  return entry?.path || '';
}

function keepBothLabel(entry, keepBothName) {
  const name = keepBothName || entry?.name || 'Untitled';
  return isDirectoryEntry(entry) ? `${name}/` : name;
}

function conflictFacts({
  entry,
  existingEntry,
  targetPath,
  keepBothName,
  incomingLabel = 'Incoming',
  mode = 'copy',
  dateFormat = 'system',
  extraFacts = [],
}) {
  const actionLabel = mode === 'move' ? 'Move' : mode === 'rename' ? 'Rename' : 'Copy';

  return [
    { label: 'Destination', value: targetPath, mono: true },
    { label: `${incomingLabel || actionLabel} Item`, value: itemName(entry) },
    { label: `${incomingLabel || actionLabel} Info`, value: entrySummary(entry, dateFormat) },
    { label: 'Existing Item', value: itemName(existingEntry) },
    { label: 'Existing Info', value: entrySummary(existingEntry, dateFormat) },
    { label: 'Type Comparison', value: compareKind(entry, existingEntry) },
    { label: 'Size Comparison', value: compareSize(entry, existingEntry) },
    { label: 'Date Comparison', value: compareModified(entry, existingEntry, dateFormat) },
    { label: 'Keep Both Name', value: keepBothLabel(entry, keepBothName), mono: true },
    { label: 'Existing At', value: itemPath(existingEntry), mono: true },
    { label: `${actionLabel} From`, value: itemPath(entry), mono: true },
    ...extraFacts,
  ];
}

function conflictApplyLabel(conflictKind, mode) {
  const itemLabel = conflictKind === 'folder' ? 'folder' : 'file';
  const modeLabel = mode === 'move' ? 'moving' : mode === 'rename' ? 'renaming' : 'copying';
  return `Use this choice for all ${itemLabel} conflicts while ${modeLabel}`;
}

function canCompareChecksums(entry, existingEntry) {
  return (
    entry?.kind === 'file' &&
    existingEntry?.kind === 'file' &&
    !isArchivePath(entry.path) &&
    !isArchivePath(existingEntry.path)
  );
}

function checksumPreview(value) {
  const hash = String(value || '');

  return hash.length > 24 ? `${hash.slice(0, 16)}...${hash.slice(-8)}` : hash;
}

function checksumFacts(comparison) {
  if (!comparison) {
    return [];
  }

  return [
    {
      label: 'Checksum Result',
      value: comparison.equal ? 'Files are identical' : 'Files differ',
    },
    {
      label: 'Incoming SHA-256',
      value: checksumPreview(comparison.leftHash),
      mono: true,
    },
    {
      label: 'Existing SHA-256',
      value: checksumPreview(comparison.rightHash),
      mono: true,
    },
  ];
}

function conditionalConflictAction(action, entry, existingEntry) {
  const comparison = modifiedComparison(entry, existingEntry);

  if (action === 'replaceNewer') {
    return comparison > 0 ? 'replace' : 'skip';
  }

  if (action === 'replaceOlder') {
    return comparison < 0 ? 'replace' : 'skip';
  }

  return action;
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

function isLocalUndoablePath(path) {
  const value = String(path || '');
  return Boolean(value) && !isRemotePath(value) && !isArchivePath(value);
}

// Undo/redo replays operations with local transfer commands; only record when
// every endpoint is a plain local path so the inverse is well-defined.
function transferIsUndoable(items) {
  return (
    Array.isArray(items) &&
    items.length > 0 &&
    items.every((item) => isLocalUndoablePath(item.from) && isLocalUndoablePath(item.to))
  );
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
    targetPath,
    keepBothName,
    conflictKind,
    mode,
    allowApplyToAll,
  }) {
    const folderConflict = conflictKind === 'folder';
    const dateFormat = store.appSettings.dateFormat;
    let extraFacts = [];

    while (true) {
      const actions = folderConflict
        ? [
            { value: 'cancel', label: 'Cancel', cancel: true },
            { value: 'skip', label: 'Skip' },
            { value: 'keepBoth', label: 'Keep Both', primary: true, default: true },
          ]
        : [
            { value: 'cancel', label: 'Cancel', cancel: true },
            { value: 'skip', label: 'Skip' },
            { value: 'keepBoth', label: 'Keep Both', primary: true, default: true },
            { value: 'replaceNewer', label: 'If Incoming Newer' },
            { value: 'replaceOlder', label: 'If Incoming Older' },
            ...(canCompareChecksums(entry, existingEntry)
              ? [{ value: 'checksum', label: 'Compare Checksum' }]
              : []),
            { value: 'replace', label: 'Replace', variant: 'danger', destructive: true },
          ];

      const result = await dialog.choice({
        title: folderConflict ? 'Folder Already Exists' : 'File Already Exists',
        message: `A ${folderConflict ? 'folder' : 'file'} named "${targetName}" already exists here.`,
        detail: folderConflict
          ? 'Keep Both creates a new folder name. Skip leaves the existing folder untouched.'
          : 'Compare metadata, keep both, skip, replace, or replace only when the incoming file is newer or older.',
        size: 'wide',
        variant: 'warning',
        icon: folderConflict ? 'folder' : 'file',
        facts: conflictFacts({
          entry,
          existingEntry,
          targetPath,
          keepBothName,
          mode,
          dateFormat,
          extraFacts,
        }),
        checkboxLabel: allowApplyToAll ? conflictApplyLabel(conflictKind, mode) : '',
        actions,
      });

      if (!result) {
        return null;
      }

      if (result.value === 'checksum') {
        try {
          const comparison = await compareFileChecksums(entry.path, existingEntry.path);
          extraFacts = checksumFacts(comparison);
        } catch (error) {
          await dialog.alert({
            title: 'Checksum Comparison Failed',
            message: error?.message || 'The files could not be compared by checksum.',
            variant: 'warning',
          });
        }

        continue;
      }

      return {
        action: result.value,
        applyToAll: Boolean(result.applyToAll),
      };
    }
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

    if (isArchivePath(targetDirectory)) {
      await dialog.alert({
        title: mode === 'move' ? 'Move Not Possible' : 'Copy Not Possible',
        message: 'Archives are read-only while browsing.',
        detail: 'Copy items out of an archive into a normal folder. Adding files to archives is not supported yet.',
        variant: 'warning',
      });
      return null;
    }

    if (mode === 'move' && sourceEntries.some((entry) => isArchivePath(entry.path))) {
      await dialog.alert({
        title: 'Move Not Possible',
        message: 'Archive contents cannot be moved.',
        detail: 'Use copy to extract items from the archive.',
        variant: 'warning',
      });
      return null;
    }

    const targetEntries = await entriesByName(targetDirectory);
    const invalid = [];
    const skipped = [];
    const items = [];
    const targetIsRemote = isRemotePath(targetDirectory);
    const effectiveSymlinkMode = targetIsRemote && !symlinkMode ? 'follow' : symlinkMode;
    const resolvedSymlinkMode = await chooseSymlinkMode(sourceEntries, mode, effectiveSymlinkMode);
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
          const keepBothName = uniqueTargetName(targetName, targetEntries, entry);
          let resolution = conflictPolicies[conflictKind];

          if (!resolution) {
            resolution = await chooseConflictResolution({
              entry,
              existingEntry,
              targetName,
              targetPath,
              keepBothName,
              conflictKind,
              mode,
              allowApplyToAll: sourceEntries.length > 1,
            });

            if (!resolution) {
              return null;
            }

            if (resolution.applyToAll) {
              conflictPolicies[conflictKind] = resolution;
            }
          }

          const resolvedAction = conditionalConflictAction(resolution.action, entry, existingEntry);

          if (resolvedAction === 'skip') {
            skipped.push({ entry, reason: 'conflict' });
            continue;
          }

          if (resolvedAction === 'keepBoth') {
            targetName = keepBothName;
            targetPath = joinPath(targetDirectory, targetName);
          } else if (resolvedAction === 'replace') {
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
      if (skipped.length > 0) {
        await dialog.alert({
          title: 'Items Skipped',
          message: `${itemLabel(skipped.length)} were skipped because of name conflicts.`,
          detail: namesPreview(skipped.map((item) => item.entry)),
          variant: 'warning',
        });
      }

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

    if (isArchivePath(targetDirectory) || paths.some((path) => isArchivePath(path))) {
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
      remotePaths: [
        targetDirectory,
        ...sourceEntries.map((entry) => entry.path),
        ...runItems.flatMap((item) => [item.from, item.to]),
      ],
      retryAction,
    });

    try {
      if (mode === 'move') {
        await moveItems(runItems, jobId);
      } else {
        await copyItems(runItems, jobId);
      }

      // Re-key color tags before the reload so the moved file keeps its dot
      // immediately (no-op when nothing was tagged).
      if (mode === 'move') {
        await store.relocateFileTags(runItems.map((item) => ({ from: item.from, to: item.to }))).catch(() => {});
      }

      await Promise.all(
        [...new Set(touchedDirectories.filter(Boolean))]
          .map((path) => store.reloadDirectoryInPanes(path)),
      );
      store.completeQueueJob(jobId, `${itemText} ${mode === 'move' ? 'moved' : 'copied'}`);

      if (transferIsUndoable(runItems)) {
        if (mode === 'move') {
          store.recordHistory({
            kind: 'move',
            label: `${itemText} moved`,
            items: runItems.map((item) => ({
              from: item.from,
              to: item.to,
              symlinkMode: item.symlinkMode,
            })),
            directories: [...new Set(touchedDirectories.filter(Boolean))],
          });
        } else {
          // Only the items we newly created are safe to delete on undo; items
          // that replaced an existing file can't restore the original.
          const createdPaths = runItems
            .filter((item) => !item.overwrite)
            .map((item) => item.to);

          if (createdPaths.length > 0) {
            store.recordHistory({
              kind: 'copy',
              label: `${itemText} copied`,
              items: runItems.map((item) => ({ ...item })),
              createdPaths,
              directories: [targetDirectory].filter(Boolean),
            });
          }
        }
      }
    } catch (error) {
      // Operations now continue past individual item failures, so some items
      // may have completed before the error. Refresh so the panes match disk.
      await Promise.allSettled(
        [...new Set(touchedDirectories.filter(Boolean))]
          .map((path) => store.reloadDirectoryInPanes(path)),
      );

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

    if (isArchivePath(entry.path)) {
      await dialog.alert({
        title: 'Rename Not Possible',
        message: 'Archive contents are read-only while browsing.',
        variant: 'warning',
      });
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
      const conflictKind = shouldUseFolderConflictActions(entry, existingEntry) ? 'folder' : 'file';
      const keepBothName = uniqueTargetName(targetName, targetEntries, entry);
      const result = await chooseConflictResolution({
        entry,
        existingEntry,
        targetName,
        targetPath,
        keepBothName,
        conflictKind,
        mode: 'rename',
        allowApplyToAll: false,
      });

      if (!result) {
        return false;
      }

      const resolvedAction = conditionalConflictAction(result.action, entry, existingEntry);

      if (resolvedAction === 'skip') {
        return false;
      }

      if (resolvedAction === 'keepBoth') {
        resolvedTargetPath = joinPath(targetDirectory, keepBothName);
      }
    }

    await renameItem(entry.path, resolvedTargetPath);
    // Keep any color tag attached to the renamed item (awaited so the caller's
    // reload shows the dot in its new place). No-op when untagged.
    await store.relocateFileTags([{ from: entry.path, to: resolvedTargetPath }]).catch(() => {});

    if (isLocalUndoablePath(entry.path) && isLocalUndoablePath(resolvedTargetPath)) {
      store.recordHistory({
        kind: 'rename',
        label: `"${entry.name}" renamed`,
        from: entry.path,
        to: resolvedTargetPath,
        directories: [targetDirectory],
      });
    }

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
