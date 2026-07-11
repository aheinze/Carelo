const WINDOWS_DRIVE_ROOT = /^[A-Za-z]:[\\/]$/;
const WINDOWS_DRIVE_PATH = /^[A-Za-z]:[\\/]/;
const WINDOWS_UNC_PATH = /^\\\\/;

function isWindowsLocalPath(path) {
  const value = String(path || '');
  return WINDOWS_DRIVE_PATH.test(value) || WINDOWS_UNC_PATH.test(value);
}

function comparableHierarchyPath(path, windowsStyle) {
  let value = String(path || '').trim();

  if (windowsStyle) {
    value = value.replace(/\\/g, '/').toLowerCase();
  }

  if (value === '/') {
    return value;
  }

  if (/^[a-z]:\/$/i.test(value)) {
    return value;
  }

  return value.replace(/\/+$/, '');
}

export function isSameOrChildLocalPath(path, parent) {
  const windowsStyle = isWindowsLocalPath(path) || isWindowsLocalPath(parent);
  const child = comparableHierarchyPath(path, windowsStyle);
  const base = comparableHierarchyPath(parent, windowsStyle);

  if (!child || !base) {
    return false;
  }

  const boundary = base.endsWith('/') ? base : `${base}/`;
  return child === base || child.startsWith(boundary);
}

export function parentLocalPath(path, fallback = '~') {
  const value = String(path || '').trim();

  if (!value) {
    return fallback;
  }

  if (value === '/' || value === '~' || WINDOWS_DRIVE_ROOT.test(value)) {
    return value;
  }

  const usesWindowsSeparators = WINDOWS_DRIVE_PATH.test(value) || WINDOWS_UNC_PATH.test(value);
  const cleanPath = usesWindowsSeparators
    ? value.replace(/[\\/]+$/, '')
    : value.replace(/\/+$/, '');
  const separatorIndex = usesWindowsSeparators
    ? Math.max(cleanPath.lastIndexOf('/'), cleanPath.lastIndexOf('\\'))
    : cleanPath.lastIndexOf('/');

  if (separatorIndex < 0) {
    return fallback;
  }

  if (separatorIndex === 0) {
    return cleanPath[0];
  }

  if (separatorIndex === 2 && /^[A-Za-z]:/.test(cleanPath)) {
    return cleanPath.slice(0, 3);
  }

  return cleanPath.slice(0, separatorIndex) || fallback;
}
