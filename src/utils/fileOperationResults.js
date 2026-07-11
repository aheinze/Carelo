const COMPLETED_STATUS = 'completed';
const SAFE_RETRY_STATUSES = new Set(['failed', 'cancelled', 'notStarted']);

function itemsFrom(value) {
  if (Array.isArray(value)) {
    return value;
  }

  return extractFileOperationBatch(value)?.items || [];
}

function orderedItems(value) {
  return [...itemsFrom(value)].sort((left, right) => left.index - right.index);
}

/**
 * Returns a batch from either a successful command result or a thrown error.
 */
export function extractFileOperationBatch(value) {
  if (value && Array.isArray(value.items)) {
    return value;
  }

  if (value?.batch && Array.isArray(value.batch.items)) {
    return value.batch;
  }

  return null;
}

export function listCompletedFileOperationItems(value) {
  return orderedItems(value).filter((item) => item.status === COMPLETED_STATUS);
}

export function isSafeRetryFileOperationItem(item) {
  return item?.affected === false && SAFE_RETRY_STATUSES.has(item.status);
}

export function listSafeRetryFileOperationItems(value) {
  return orderedItems(value).filter(isSafeRetryFileOperationItem);
}

export function isSafeSudoRetryFileOperationItem(command, item) {
  const permissionDenied = Array.isArray(item?.errors)
    && item.errors.some((error) => error?.code === 'permission_denied');

  if (!permissionDenied) {
    return false;
  }

  if (item?.affected === false && item.status === 'failed') {
    return true;
  }

  // Recursive delete is idempotent: sudo can safely finish a directory after
  // an unprivileged remove_dir_all deleted some children and then failed.
  return command === 'delete_items' && item?.affected === true && item.status === 'partial';
}

/**
 * Pairs operation results with their original input. Index identity is retained
 * even when paths are duplicated or backend work completed out of order.
 */
export function mapFileOperationItemsToInputs(value, originalInputs) {
  const inputs = Array.isArray(originalInputs) ? originalInputs : [];

  return orderedItems(value).map((result) => ({
    result,
    input: inputs[result.index],
  }));
}

/**
 * Builds a request subset and the index map needed to merge its eventual batch
 * result back into the original batch. Safe retry items are selected by default.
 */
export function buildFileOperationSubset(
  value,
  originalInputs,
  select = isSafeRetryFileOperationItem,
) {
  const inputs = Array.isArray(originalInputs) ? originalInputs : [];
  const selectedResults = orderedItems(value)
    .filter((result) => select(result, inputs[result.index]));

  return {
    items: selectedResults.map((result) => inputs[result.index]),
    originalIndices: selectedResults.map((result) => result.index),
    results: selectedResults,
  };
}

/**
 * Replaces results in an original batch with the results from a subset request.
 * Subset indices are local to that request and are translated with originalIndices.
 */
export function mergeFileOperationSubsetBatch(value, subsetValue, originalIndices) {
  const mergedByIndex = new Map(
    orderedItems(value).map((item) => [item.index, item]),
  );
  const indexMap = Array.isArray(originalIndices) ? originalIndices : [];

  for (const subsetItem of orderedItems(subsetValue)) {
    const originalIndex = indexMap[subsetItem.index];

    if (!Number.isInteger(originalIndex)) {
      throw new RangeError(`Missing original index for subset item ${subsetItem.index}`);
    }

    mergedByIndex.set(originalIndex, {
      ...subsetItem,
      index: originalIndex,
    });
  }

  const items = [...mergedByIndex.values()].sort((left, right) => left.index - right.index);

  return {
    items,
    cancelled: Boolean(extractFileOperationBatch(subsetValue)?.cancelled)
      || items.some((item) => item.status === 'cancelled'),
  };
}

/**
 * Creates the aggregate rejection used by copy, move, and delete commands while
 * retaining the structured batch for exact retry and history decisions.
 */
export function buildFileOperationBatchError(value) {
  const sourceBatch = extractFileOperationBatch(value);
  const items = orderedItems(value);
  const batch = {
    items,
    cancelled: Boolean(sourceBatch?.cancelled)
      || items.some((item) => item.status === 'cancelled'),
  };
  const incompleteItems = batch.items.filter((item) => item.status !== COMPLETED_STATUS);
  const underlyingErrors = incompleteItems.flatMap((item) => (
    Array.isArray(item.errors) ? item.errors : []
  ));
  const cancellationError = underlyingErrors
    .find((error) => error?.code === 'operation_cancelled');
  const hasCancellation = batch.cancelled || cancellationError
    || incompleteItems.some((item) => item.status === 'cancelled');

  let detail = null;
  let code = 'operation_partial_failure';
  let message = `${incompleteItems.length} file operation item${incompleteItems.length === 1 ? '' : 's'} did not complete.`;

  if (hasCancellation) {
    detail = cancellationError;
    code = 'operation_cancelled';
    message = detail?.message || 'File operation cancelled.';
  } else if (underlyingErrors.length === 1) {
    [detail] = underlyingErrors;
    code = detail?.code || code;
    message = detail?.message || message;
  }

  const error = new Error(message);
  error.code = code;
  error.batch = batch;

  if (detail && Object.hasOwn(detail, 'path')) {
    error.path = detail.path;
  }

  return error;
}

function historyEntrySubset(entry, indices, makeId) {
  let ordered = [...new Set(indices)].sort((left, right) => left - right);

  if (entry.kind === 'move') {
    ordered = ordered.filter((index) => Boolean(entry.items?.[index]));
  } else if (entry.kind === 'copy') {
    // Treat the request and created path as one pair so an older malformed
    // history entry can never shift redo inputs onto the wrong destination.
    ordered = ordered.filter((index) => (
      Boolean(entry.items?.[index]) && Boolean(entry.createdPaths?.[index])
    ));
  } else if (entry.kind === 'delete') {
    ordered = ordered.filter((index) => Boolean(entry.paths?.[index]));
  }

  if (ordered.length === 0) {
    return null;
  }

  const subset = { ...entry, id: makeId() };

  if (entry.kind === 'move') {
    subset.items = ordered.map((index) => entry.items[index]);
  } else if (entry.kind === 'copy') {
    subset.items = ordered.map((index) => entry.items[index]);
    subset.createdPaths = ordered.map((index) => entry.createdPaths[index]);
  } else if (entry.kind === 'delete') {
    subset.paths = ordered.map((index) => entry.paths[index]);
  }

  return subset;
}

export function splitFileOperationHistoryEntry(entry, value, makeId = () => entry?.id) {
  const batch = extractFileOperationBatch(value);

  if (!entry || !batch) {
    return { completed: null, pending: entry || null };
  }

  return {
    completed: historyEntrySubset(
      entry,
      listCompletedFileOperationItems(batch).map((item) => item.index),
      makeId,
    ),
    pending: historyEntrySubset(
      entry,
      listSafeRetryFileOperationItems(batch).map((item) => item.index),
      makeId,
    ),
  };
}
