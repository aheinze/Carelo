import test from 'node:test';
import assert from 'node:assert/strict';
import {
  buildFileOperationBatchError,
  buildFileOperationSubset,
  extractFileOperationBatch,
  isSafeSudoRetryFileOperationItem,
  listCompletedFileOperationItems,
  listSafeRetryFileOperationItems,
  mapFileOperationItemsToInputs,
  mergeFileOperationSubsetBatch,
  splitFileOperationHistoryEntry,
} from '../src/utils/fileOperationResults.js';

function result(index, status, options = {}) {
  return {
    index,
    from: options.from || `/source/${index}`,
    to: options.to || `/target/${index}`,
    status,
    affected: options.affected ?? status === 'completed',
    errors: options.errors || [],
  };
}

test('extracts batches from successful results and errors', () => {
  const batch = { items: [result(0, 'completed')] };
  const error = new Error('failed');
  error.batch = batch;

  assert.equal(extractFileOperationBatch(batch), batch);
  assert.equal(extractFileOperationBatch(error), batch);
  assert.equal(extractFileOperationBatch(null), null);
});

test('builds a safe subset and merges out-of-order results in original order', () => {
  const originalInputs = [
    { from: '/source/a', to: '/target/a' },
    { from: '/source/b', to: '/target/b' },
    { from: '/source/c', to: '/target/c' },
  ];
  const originalBatch = {
    cancelled: false,
    items: [
      result(2, 'failed', { affected: false }),
      result(0, 'failed', { affected: false }),
      result(1, 'completed'),
    ],
  };
  const subset = buildFileOperationSubset(originalBatch, originalInputs);
  const retriedBatch = {
    cancelled: true,
    items: [
      result(1, 'completed', { from: '/source/c', to: '/target/c' }),
      result(0, 'completed', { from: '/source/a', to: '/target/a' }),
    ],
  };

  assert.deepEqual(subset.originalIndices, [0, 2]);
  assert.deepEqual(subset.items, [originalInputs[0], originalInputs[2]]);

  const merged = mergeFileOperationSubsetBatch(
    originalBatch,
    retriedBatch,
    subset.originalIndices,
  );

  assert.deepEqual(merged.items.map((item) => item.index), [0, 1, 2]);
  assert.deepEqual(merged.items.map((item) => item.status), [
    'completed',
    'completed',
    'completed',
  ]);
  assert.deepEqual(merged.items.map((item) => item.from), [
    '/source/a',
    '/source/1',
    '/source/c',
  ]);
  assert.equal(merged.cancelled, true);
});

test('safe retry includes only clean failed, cancelled, and not-started items', () => {
  const batch = {
    items: [
      result(5, 'notStarted', { affected: false }),
      result(0, 'failed', { affected: false }),
      result(1, 'failed', { affected: true }),
      result(2, 'partial', { affected: false }),
      result(3, 'cancelled', { affected: false }),
      result(4, 'completed'),
    ],
  };

  assert.deepEqual(
    listSafeRetryFileOperationItems(batch).map((item) => item.index),
    [0, 3, 5],
  );
});

test('sudo permits an affected permission failure only for idempotent delete', () => {
  const partialDelete = result(0, 'partial', {
    affected: true,
    errors: [{ code: 'permission_denied', message: 'Permission denied', path: '/protected' }],
  });

  assert.equal(isSafeSudoRetryFileOperationItem('delete_items', partialDelete), true);
  assert.equal(isSafeSudoRetryFileOperationItem('copy_items', partialDelete), false);
  assert.equal(isSafeSudoRetryFileOperationItem('move_items', partialDelete), false);
});

test('preserves a lone permission error code, message, and path', () => {
  const batch = {
    cancelled: false,
    items: [
      result(0, 'completed'),
      result(1, 'failed', {
        affected: false,
        errors: [{
          code: 'permission_denied',
          message: 'Permission denied',
          path: '/protected/file.txt',
        }],
      }),
    ],
  };
  const error = buildFileOperationBatchError(batch);

  assert.equal(error.code, 'permission_denied');
  assert.equal(error.message, 'Permission denied');
  assert.equal(error.path, '/protected/file.txt');
  assert.deepEqual(error.batch, batch);
});

test('cancellation dominates other failures in the aggregate error', () => {
  const batch = {
    items: [
      result(0, 'failed', {
        affected: false,
        errors: [{ code: 'permission_denied', message: 'Permission denied', path: '/a' }],
      }),
      result(1, 'cancelled', { affected: false }),
    ],
  };
  const error = buildFileOperationBatchError(batch);

  assert.equal(error.code, 'operation_cancelled');
  assert.equal(error.message, 'File operation cancelled.');
  assert.equal(error.batch.cancelled, true);
  assert.equal(error.batch.items.length, 2);
});

test('uses a partial-failure error when more than one underlying error remains', () => {
  const batch = {
    items: [
      result(0, 'failed', {
        affected: false,
        errors: [{ code: 'permission_denied', message: 'Permission denied', path: '/a' }],
      }),
      result(1, 'failed', {
        affected: false,
        errors: [{ code: 'disk_full', message: 'Disk full', path: '/b' }],
      }),
    ],
  };
  const error = buildFileOperationBatchError(batch);

  assert.equal(error.code, 'operation_partial_failure');
  assert.equal(error.path, undefined);
  assert.equal(error.batch.items.length, 2);
});

test('lists completed results and maps them to original inputs by index', () => {
  const originalInputs = [
    { from: '/duplicate', marker: 'first' },
    { from: '/other', marker: 'second' },
    { from: '/duplicate', marker: 'third' },
  ];
  const batch = {
    items: [
      result(2, 'completed'),
      result(0, 'completed'),
      result(1, 'failed', { affected: false }),
    ],
  };
  const completed = listCompletedFileOperationItems(batch);
  const mapped = mapFileOperationItemsToInputs(completed, originalInputs);

  assert.deepEqual(completed.map((item) => item.index), [0, 2]);
  assert.deepEqual(mapped.map(({ input }) => input.marker), ['first', 'third']);
  assert.deepEqual(mapped.map(({ result: item }) => item.index), [0, 2]);
});

test('splits copy history with aligned items and created paths and fresh ids', () => {
  let nextId = 0;
  const entry = {
    id: 'original',
    kind: 'copy',
    items: [{ key: 'a' }, { key: 'b' }, { key: 'c' }],
    createdPaths: ['/a', '/b', '/c'],
  };
  const batch = {
    items: [
      result(0, 'completed'),
      result(1, 'failed', { affected: false }),
      result(2, 'partial', { affected: true }),
    ],
  };
  const split = splitFileOperationHistoryEntry(entry, batch, () => `split-${nextId++}`);

  assert.equal(split.completed.id, 'split-0');
  assert.deepEqual(split.completed.items, [{ key: 'a' }]);
  assert.deepEqual(split.completed.createdPaths, ['/a']);
  assert.equal(split.pending.id, 'split-1');
  assert.deepEqual(split.pending.items, [{ key: 'b' }]);
  assert.deepEqual(split.pending.createdPaths, ['/b']);
});

test('keeps malformed copy history pairs aligned when splitting', () => {
  const split = splitFileOperationHistoryEntry({
    id: 'copy',
    kind: 'copy',
    items: [{ key: 'a' }, { key: 'b' }],
    createdPaths: ['/a'],
  }, {
    items: [result(0, 'completed'), result(1, 'completed')],
  }, () => 'split-copy');

  assert.deepEqual(split.completed.items, [{ key: 'a' }]);
  assert.deepEqual(split.completed.createdPaths, ['/a']);
  assert.equal(split.pending, null);
});

test('splits move and delete history while dropping affected outcomes', () => {
  const batch = {
    items: [
      result(0, 'completed'),
      result(1, 'notStarted', { affected: false }),
      result(2, 'partial', { affected: true }),
    ],
  };
  const move = splitFileOperationHistoryEntry({
    id: 'move',
    kind: 'move',
    items: [{ key: 'a' }, { key: 'b' }, { key: 'c' }],
  }, batch, () => 'new-move');
  const deletion = splitFileOperationHistoryEntry({
    id: 'delete',
    kind: 'delete',
    paths: ['/a', '/b', '/c'],
  }, batch, () => 'new-delete');

  assert.deepEqual(move.completed.items, [{ key: 'a' }]);
  assert.deepEqual(move.pending.items, [{ key: 'b' }]);
  assert.deepEqual(deletion.completed.paths, ['/a']);
  assert.deepEqual(deletion.pending.paths, ['/b']);
});
