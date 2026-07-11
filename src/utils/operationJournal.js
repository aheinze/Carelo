const JOURNAL_ID_LIMIT = 160;
const JOURNAL_OPERATION_LIMIT = 96;
const JOURNAL_LABEL_LIMIT = 240;
const JOURNAL_DETAIL_LIMIT = 4096;

const SUPPORTED_STATUSES = new Set([
  'queued',
  'pending',
  'running',
  'pausing',
  'paused',
  'cancelling',
  'completed',
  'failed',
  'cancelled',
  'interrupted',
]);

const TERMINAL_STATUSES = new Set(['completed', 'failed', 'cancelled', 'interrupted']);
const STATE_FIELDS = ['operation', 'label', 'detail', 'status'];
const CREDENTIAL_URL_PATTERN = /[a-z][a-z0-9+.-]*:\/\/[^/\s:@]+:[^@\s/]+@/i;
const SECRET_ASSIGNMENT_PATTERN = /\b(?:password|passwd|secret|token|api[-_ ]?key|access[-_ ]?key)\s*[:=]\s*\S+/i;

function truncateUtf8(value, maxBytes) {
  const text = String(value || '');
  let bytes = 0;
  let result = '';

  for (const character of text) {
    const characterBytes = new TextEncoder().encode(character).byteLength;

    if (bytes + characterBytes > maxBytes) {
      break;
    }

    result += character;
    bytes += characterBytes;
  }

  return result;
}

function containsSensitiveValue(value) {
  const text = String(value || '').trim();

  return (
    CREDENTIAL_URL_PATTERN.test(text)
    || /(?:^|\s)bearer\s+\S+/i.test(text)
    || /(?:^|\s)basic\s+\S+/i.test(text)
    || SECRET_ASSIGNMENT_PATTERN.test(text)
    || (text.includes('-----BEGIN ') && text.includes('PRIVATE KEY-----'))
  );
}

function safeText(value, maxBytes, fallback = '') {
  const text = String(value || '')
    .replace(/[\u0000-\u001f\u007f]/g, ' ')
    .trim();

  if (!text || containsSensitiveValue(text)) {
    return fallback;
  }

  return truncateUtf8(text, maxBytes).trim() || fallback;
}

function safeIdentifier(value, fallback, maxBytes) {
  const identifier = String(value || '')
    .trim()
    .replace(/[^a-zA-Z0-9_.:-]+/g, '-')
    .replace(/^-+|-+$/g, '');

  return truncateUtf8(identifier || fallback, maxBytes);
}

function finiteNonNegative(value) {
  const number = Number(value);

  if (!Number.isFinite(number) || number < 0) {
    return 0;
  }

  return Math.min(Math.round(number), Number.MAX_SAFE_INTEGER);
}

function finiteProgress(value) {
  const number = Number(value);

  if (!Number.isFinite(number)) {
    return null;
  }

  return Math.max(0, Math.min(1, number));
}

function normalizeTimestamp(value, fallback = Date.now()) {
  const timestamp = Number(value);

  if (!Number.isFinite(timestamp) || timestamp < 0) {
    return Math.max(0, Math.round(fallback));
  }

  return Math.round(timestamp);
}

export function isTerminalOperationJournalStatus(status) {
  return TERMINAL_STATUSES.has(String(status || '').toLowerCase());
}

export function nextOperationJournalTimestamp(previous, candidate = Date.now()) {
  const normalizedCandidate = normalizeTimestamp(candidate);
  const normalizedPrevious = Number.isFinite(Number(previous))
    ? normalizeTimestamp(previous)
    : -1;

  return Math.max(normalizedCandidate, normalizedPrevious + 1);
}

export function operationJournalUpdateMode(previous, next) {
  if (isTerminalOperationJournalStatus(next?.status)) {
    return 'terminal';
  }

  if (!previous || STATE_FIELDS.some((field) => previous?.[field] !== next?.[field])) {
    return 'state';
  }

  return 'progress';
}

function normalizedStatus(status, fallback = 'running') {
  const value = String(status || '').trim().toLowerCase();
  return SUPPORTED_STATUSES.has(value) ? value : fallback;
}

function commonJournalInput(source, updatedAt) {
  const createdAt = normalizeTimestamp(source?.createdAt, updatedAt);
  const normalizedUpdatedAt = Math.max(createdAt, normalizeTimestamp(updatedAt, createdAt));
  const status = normalizedStatus(source?.status);

  return {
    id: safeIdentifier(source?.id, `operation-${createdAt}`, JOURNAL_ID_LIMIT),
    operation: safeIdentifier(source?.operation, 'operation', JOURNAL_OPERATION_LIMIT),
    label: safeText(source?.label, JOURNAL_LABEL_LIMIT, 'File operation'),
    detail: safeText(source?.detail, JOURNAL_DETAIL_LIMIT),
    status,
    createdAt,
    updatedAt: normalizedUpdatedAt,
    finishedAt: isTerminalOperationJournalStatus(status) ? normalizedUpdatedAt : null,
  };
}

export function serializeQueueJobForJournal(job, updatedAt = Date.now()) {
  const input = commonJournalInput(job, updatedAt);
  const path = safeText(job?.currentPath, JOURNAL_DETAIL_LIMIT);

  return {
    ...input,
    payload: {
      kind: 'queue',
      ...(path ? { path } : {}),
    },
    progress: {
      progress: finiteProgress(job?.progress),
      currentProgress: finiteProgress(job?.currentProgress),
      processedBytes: finiteNonNegative(job?.processedBytes),
      totalBytes: finiteNonNegative(job?.totalBytes),
      processedEntries: finiteNonNegative(job?.processedEntries),
      totalEntries: finiteNonNegative(job?.totalEntries),
      currentBytes: finiteNonNegative(job?.currentBytes),
      currentTotalBytes: finiteNonNegative(job?.currentTotalBytes),
    },
  };
}

export function serializeOperationLogForJournal(entry, updatedAt = Date.now()) {
  if (!isTerminalOperationJournalStatus(entry?.status)) {
    return null;
  }

  const input = commonJournalInput(entry, updatedAt);
  const path = safeText(entry?.path, JOURNAL_DETAIL_LIMIT);

  return {
    ...input,
    payload: {
      kind: 'log',
      ...(path ? { path } : {}),
    },
    progress: {},
  };
}

function restoredStatus(status) {
  const normalized = normalizedStatus(status, 'interrupted');
  return isTerminalOperationJournalStatus(normalized) ? normalized : 'interrupted';
}

function restoredDetail(entry, status) {
  const detail = safeText(entry?.detail, JOURNAL_DETAIL_LIMIT);

  if (status !== 'interrupted') {
    return detail;
  }

  const explanation = 'Carelo closed before this operation finished. It was not resumed.';
  return detail ? `${detail} · ${explanation}` : explanation;
}

function journalEntryIdentity(entry) {
  return String(entry?.journalId || entry?.jobId || entry?.id || '');
}

export function restoreOperationLogFromJournal(entries, existingEntries = [], limit = 120) {
  const existing = Array.isArray(existingEntries) ? existingEntries : [];
  const seen = new Set(existing.map(journalEntryIdentity).filter(Boolean));
  const restored = [];

  for (const entry of Array.isArray(entries) ? entries : []) {
    const journalId = safeIdentifier(entry?.id, '', JOURNAL_ID_LIMIT);

    if (!journalId || seen.has(journalId)) {
      continue;
    }

    const status = restoredStatus(entry?.status);
    const kind = entry?.payload?.kind === 'queue' ? 'queue' : 'log';
    const createdAt = normalizeTimestamp(
      entry?.finishedAt ?? entry?.updatedAt ?? entry?.createdAt,
    );
    const path = safeText(entry?.payload?.path, JOURNAL_DETAIL_LIMIT);

    restored.push({
      id: `journal:${journalId}`,
      journalId,
      jobId: kind === 'queue' ? journalId : null,
      operation: safeIdentifier(entry?.operation, 'operation', JOURNAL_OPERATION_LIMIT),
      label: safeText(entry?.label, JOURNAL_LABEL_LIMIT, 'File operation'),
      detail: restoredDetail(entry, status),
      status,
      path,
      createdAt,
      restored: true,
    });
    seen.add(journalId);
  }

  return [...existing, ...restored]
    .sort((left, right) => Number(right.createdAt || 0) - Number(left.createdAt || 0))
    .slice(0, Math.max(0, Number(limit) || 0));
}
