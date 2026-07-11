import test from 'node:test';
import assert from 'node:assert/strict';
import {
  nextOperationJournalTimestamp,
  operationJournalUpdateMode,
  restoreOperationLogFromJournal,
  serializeOperationLogForJournal,
  serializeQueueJobForJournal,
} from '../src/utils/operationJournal.js';

test('queue serialization keeps only a bounded non-secret snapshot', () => {
  const serialized = serializeQueueJobForJournal({
    id: 'job/unsafe id',
    operation: 'copy items',
    label: 'Copy files',
    detail: 'Bearer should-not-be-persisted',
    status: 'running',
    currentPath: 'https://user:password@example.test/private',
    processedBytes: 12.7,
    totalBytes: 100,
    progress: 0.127,
    password: 'never-store-me',
    accessToken: 'never-store-me-either',
    retryAction() {},
    createdAt: 100,
  }, 101);

  assert.equal(serialized.id, 'job-unsafe-id');
  assert.equal(serialized.operation, 'copy-items');
  assert.equal(serialized.detail, '');
  assert.deepEqual(serialized.payload, { kind: 'queue' });
  assert.equal(serialized.progress.processedBytes, 13);
  assert.equal(serialized.progress.progress, 0.127);
  assert.equal(serialized.finishedAt, null);

  const json = JSON.stringify(serialized);
  assert.doesNotMatch(json, /password|accessToken|retryAction|never-store/i);
});

test('terminal log serialization excludes live-only state and sets finished time', () => {
  const serialized = serializeOperationLogForJournal({
    id: 'log-1',
    operation: 'remote-edit',
    label: 'Remote edit synced',
    detail: 'notes.txt',
    path: 'remote://docs/notes.txt',
    status: 'completed',
    createdAt: 200,
    callback() {},
  }, 205);

  assert.deepEqual(serialized.payload, {
    kind: 'log',
    path: 'remote://docs/notes.txt',
  });
  assert.deepEqual(serialized.progress, {});
  assert.equal(serialized.updatedAt, 205);
  assert.equal(serialized.finishedAt, 205);
  assert.equal(serializeOperationLogForJournal({ status: 'running' }, 1), null);
});

test('serialized text respects backend UTF-8 byte limits', () => {
  const serialized = serializeQueueJobForJournal({
    id: 'utf8-job',
    operation: 'copy',
    label: '🧳'.repeat(100),
    detail: '文'.repeat(2000),
    status: 'running',
    createdAt: 1,
  }, 2);

  assert.ok(Buffer.byteLength(serialized.label, 'utf8') <= 240);
  assert.ok(Buffer.byteLength(serialized.detail, 'utf8') <= 4096);
});

test('journal timestamps increase even when the clock does not', () => {
  assert.equal(nextOperationJournalTimestamp(undefined, 500), 500);
  assert.equal(nextOperationJournalTimestamp(500, 500), 501);
  assert.equal(nextOperationJournalTimestamp(501, 499), 502);
});

test('queue update classification separates state, progress, and terminal writes', () => {
  const running = {
    operation: 'copy',
    label: 'Copying',
    detail: '',
    status: 'running',
    processedBytes: 1,
  };

  assert.equal(operationJournalUpdateMode(null, running), 'state');
  assert.equal(operationJournalUpdateMode(running, { ...running, processedBytes: 2 }), 'progress');
  assert.equal(operationJournalUpdateMode(running, { ...running, status: 'paused' }), 'state');
  assert.equal(operationJournalUpdateMode(running, { ...running, status: 'completed' }), 'terminal');
});

test('restoration deduplicates rows and exposes unfinished work as non-retryable interruption', () => {
  const existing = [{
    id: 'runtime-log',
    jobId: 'job-existing',
    label: 'Already visible',
    status: 'completed',
    createdAt: 400,
  }];
  const restored = restoreOperationLogFromJournal([
    {
      id: 'job-existing',
      operation: 'copy',
      label: 'Duplicate',
      status: 'completed',
      payload: { kind: 'queue' },
      createdAt: 100,
      updatedAt: 200,
      finishedAt: 200,
    },
    {
      id: 'job-interrupted',
      operation: 'move',
      label: 'Move files',
      detail: 'Moving documents',
      status: 'running',
      payload: { kind: 'queue', path: '/home/user/Documents' },
      createdAt: 300,
      updatedAt: 350,
    },
  ], existing, 120);

  assert.equal(restored.length, 2);
  assert.equal(restored[0].id, 'runtime-log');
  assert.equal(restored[1].journalId, 'job-interrupted');
  assert.equal(restored[1].status, 'interrupted');
  assert.match(restored[1].detail, /was not resumed/i);
  assert.equal(restored[1].path, '/home/user/Documents');
  assert.equal('retryAction' in restored[1], false);
});

test('restoration sorts newest first and keeps the requested log limit', () => {
  const entries = Array.from({ length: 5 }, (_, index) => ({
    id: `log-${index}`,
    operation: 'directory',
    label: `Log ${index}`,
    status: 'failed',
    payload: { kind: 'log' },
    createdAt: index,
    updatedAt: index,
    finishedAt: index,
  }));

  const restored = restoreOperationLogFromJournal(entries, [], 3);
  assert.deepEqual(restored.map((entry) => entry.journalId), ['log-4', 'log-3', 'log-2']);
});
