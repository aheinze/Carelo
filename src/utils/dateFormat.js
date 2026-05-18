export const DATE_FORMAT_OPTIONS = [
  { value: 'system', label: 'System Default' },
  { value: 'iso', label: '2026-05-18' },
  { value: 'european', label: '18.05.2026' },
  { value: 'us', label: '05/18/2026' },
  { value: 'long', label: 'May 18, 2026' },
];

const DATE_FORMAT_VALUES = new Set(DATE_FORMAT_OPTIONS.map((option) => option.value));

function pad(value) {
  return String(value).padStart(2, '0');
}

function dateFromUnixSeconds(timestamp) {
  const value = Number(timestamp);

  if (!Number.isFinite(value) || value <= 0) {
    return null;
  }

  return new Date(value * 1000);
}

function timeFor(date, format) {
  if (format === 'us') {
    return new Intl.DateTimeFormat('en-US', {
      hour: 'numeric',
      minute: '2-digit',
    }).format(date);
  }

  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function normalizeDateFormat(format) {
  return DATE_FORMAT_VALUES.has(format) ? format : 'system';
}

export function formatDate(date, format = 'system', options = {}) {
  const normalizedFormat = normalizeDateFormat(format);
  const includeTime = options.includeTime === true;

  if (!(date instanceof Date) || Number.isNaN(date.getTime())) {
    return options.fallback ?? '--';
  }

  if (normalizedFormat === 'system') {
    return new Intl.DateTimeFormat(undefined, includeTime
      ? {
          year: 'numeric',
          month: '2-digit',
          day: '2-digit',
          hour: '2-digit',
          minute: '2-digit',
        }
      : {
          year: 'numeric',
          month: '2-digit',
          day: '2-digit',
        }).format(date);
  }

  if (normalizedFormat === 'long') {
    return new Intl.DateTimeFormat(undefined, includeTime
      ? {
          dateStyle: 'medium',
          timeStyle: 'short',
        }
      : {
          dateStyle: 'medium',
        }).format(date);
  }

  const year = date.getFullYear();
  const month = pad(date.getMonth() + 1);
  const day = pad(date.getDate());
  const datePart = normalizedFormat === 'european'
    ? `${day}.${month}.${year}`
    : normalizedFormat === 'us'
      ? `${month}/${day}/${year}`
      : `${year}-${month}-${day}`;

  return includeTime ? `${datePart}, ${timeFor(date, normalizedFormat)}` : datePart;
}

export function formatFileDate(timestamp, format = 'system', options = {}) {
  const date = dateFromUnixSeconds(timestamp);
  return formatDate(date, format, options);
}

export function formatFileDateTime(timestamp, format = 'system', options = {}) {
  return formatFileDate(timestamp, format, {
    ...options,
    includeTime: true,
  });
}
