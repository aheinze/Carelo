const IMAGE_EXTENSIONS = new Set([
  'apng',
  'avif',
  'bmp',
  'cur',
  'gif',
  'heic',
  'heif',
  'ico',
  'jfif',
  'jpeg',
  'jpg',
  'pjpeg',
  'pjp',
  'png',
  'svg',
  'tif',
  'tiff',
  'webp',
]);

const VIDEO_EXTENSIONS = new Set([
  '3g2',
  '3gp',
  'avi',
  'm4v',
  'mkv',
  'mov',
  'mp4',
  'mpeg',
  'mpg',
  'ogv',
  'webm',
]);

const AUDIO_EXTENSIONS = new Set([
  'aac',
  'aif',
  'aiff',
  'alac',
  'flac',
  'm4a',
  'mp3',
  'oga',
  'ogg',
  'opus',
  'wav',
  'weba',
  'wma',
]);

const PDF_EXTENSIONS = new Set(['pdf']);

const TEXT_EXTENSIONS = new Set([
  'bash',
  'c',
  'cc',
  'cfg',
  'conf',
  'cpp',
  'cs',
  'css',
  'csv',
  'env',
  'go',
  'h',
  'hpp',
  'htm',
  'html',
  'ini',
  'java',
  'js',
  'json',
  'jsx',
  'log',
  'lua',
  'md',
  'php',
  'pl',
  'properties',
  'py',
  'rb',
  'rs',
  'scss',
  'sh',
  'sql',
  'svelte',
  'swift',
  'toml',
  'ts',
  'tsx',
  'txt',
  'vue',
  'xml',
  'yaml',
  'yml',
  'zsh',
]);

export function extensionForName(name) {
  const parts = String(name || '').split('.');
  return parts.length > 1 ? parts.at(-1).toLowerCase() : '';
}

export function isImageEntry(entry) {
  return entry?.kind === 'file' && IMAGE_EXTENSIONS.has(extensionForName(entry.name));
}

export function isVideoEntry(entry) {
  return entry?.kind === 'file' && VIDEO_EXTENSIONS.has(extensionForName(entry.name));
}

export function isAudioEntry(entry) {
  return entry?.kind === 'file' && AUDIO_EXTENSIONS.has(extensionForName(entry.name));
}

export function isPdfEntry(entry) {
  return entry?.kind === 'file' && PDF_EXTENSIONS.has(extensionForName(entry.name));
}

export function isTextEntry(entry) {
  return entry?.kind === 'file' && TEXT_EXTENSIONS.has(extensionForName(entry.name));
}

export function imageTypeLabel(name) {
  const extension = extensionForName(name);

  if (!IMAGE_EXTENSIONS.has(extension)) {
    return '';
  }

  if (extension === 'jpg' || extension === 'jpeg' || extension === 'jfif') {
    return 'JPEG';
  }

  if (extension === 'tif' || extension === 'tiff') {
    return 'TIFF';
  }

  if (extension === 'svg') {
    return 'SVG';
  }

  return extension ? extension.toUpperCase() : '';
}

export function videoTypeLabel(name) {
  const extension = extensionForName(name);

  if (!VIDEO_EXTENSIONS.has(extension)) {
    return '';
  }

  if (extension === 'mp4' || extension === 'm4v') {
    return 'MPEG-4 video';
  }

  if (extension === 'mov') {
    return 'QuickTime video';
  }

  if (extension === 'webm') {
    return 'WebM video';
  }

  if (extension === 'ogv') {
    return 'Ogg video';
  }

  return extension ? `${extension.toUpperCase()} video` : '';
}

export function audioTypeLabel(name) {
  const extension = extensionForName(name);

  if (!AUDIO_EXTENSIONS.has(extension)) {
    return '';
  }

  if (extension === 'mp3') return 'MP3 audio';
  if (extension === 'm4a' || extension === 'aac') return 'AAC audio';
  if (extension === 'wav') return 'WAV audio';
  if (extension === 'flac') return 'FLAC audio';
  if (extension === 'oga' || extension === 'ogg' || extension === 'opus') return 'Ogg audio';
  if (extension === 'weba') return 'WebM audio';
  if (extension === 'aif' || extension === 'aiff') return 'AIFF audio';

  return extension ? `${extension.toUpperCase()} audio` : '';
}

export function documentTypeLabel(name) {
  const extension = extensionForName(name);

  if (extension === 'pdf') return 'PDF document';
  if (extension === 'md') return 'Markdown document';
  if (extension === 'txt') return 'Plain text';
  if (TEXT_EXTENSIONS.has(extension)) return `${extension.toUpperCase()} text`;

  return '';
}

export function videoMimeType(name) {
  const extension = extensionForName(name);

  if (extension === 'mp4' || extension === 'm4v') return 'video/mp4';
  if (extension === 'mov') return 'video/quicktime';
  if (extension === 'webm') return 'video/webm';
  if (extension === 'ogv') return 'video/ogg';
  if (extension === 'mpeg' || extension === 'mpg') return 'video/mpeg';
  if (extension === '3gp') return 'video/3gpp';
  if (extension === '3g2') return 'video/3gpp2';
  if (extension === 'avi') return 'video/x-msvideo';
  if (extension === 'mkv') return 'video/x-matroska';

  return '';
}

export function audioMimeType(name) {
  const extension = extensionForName(name);

  if (extension === 'mp3') return 'audio/mpeg';
  if (extension === 'm4a') return 'audio/mp4';
  if (extension === 'aac') return 'audio/aac';
  if (extension === 'wav') return 'audio/wav';
  if (extension === 'flac') return 'audio/flac';
  if (extension === 'oga' || extension === 'ogg') return 'audio/ogg';
  if (extension === 'opus') return 'audio/ogg; codecs=opus';
  if (extension === 'weba') return 'audio/webm';
  if (extension === 'aif' || extension === 'aiff') return 'audio/aiff';
  if (extension === 'wma') return 'audio/x-ms-wma';
  if (extension === 'alac') return 'audio/mp4';

  return '';
}
