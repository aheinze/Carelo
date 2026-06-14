<script setup>
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { useQuickLook } from '../composables/useQuickLook';
import { useFileManagerStore } from '../stores/fileManagerStore';
import hljs from 'highlight.js/lib/common';
import {
  createMediaStreamUrl,
  isRemotePath,
  localFileAssetUrl,
  readMediaPreview,
  readTextPreview,
} from '../composables/useFileOperations';
import {
  extensionForName,
  isAudioEntry,
  isImageEntry,
  isPdfEntry,
  isTextEntry,
  isVideoEntry,
} from '../utils/fileTypes';
import { fileTypeIconName } from '../utils/fileTypeIcons';
import { isArchivePath } from '../utils/archivePaths';
import { formatFileDateTime } from '../utils/dateFormat';

// Extensions whose highlight.js language name isn't the same as the extension.
const HIGHLIGHT_LANGUAGE_BY_EXTENSION = {
  vue: 'xml',
  htm: 'xml',
  svg: 'xml',
  h: 'cpp',
  hpp: 'cpp',
  cc: 'cpp',
  cxx: 'cpp',
  mjs: 'javascript',
  cjs: 'javascript',
  jsx: 'javascript',
  tsx: 'typescript',
  toml: 'ini',
  yml: 'yaml',
  sh: 'bash',
  zsh: 'bash',
  rs: 'rust',
  py: 'python',
  rb: 'ruby',
  kt: 'kotlin',
  cs: 'csharp',
};
const HIGHLIGHT_MAX_CHARS = 150_000;

function highlightLanguage(name) {
  const ext = extensionForName(name);

  if (!ext) {
    return '';
  }

  const candidate = HIGHLIGHT_LANGUAGE_BY_EXTENSION[ext] || ext;
  return hljs.getLanguage(candidate) ? candidate : '';
}

function highlightCode(code, name) {
  if (!code || code.length > HIGHLIGHT_MAX_CHARS) {
    return '';
  }

  // Highlight only recognized code extensions; plain prose (txt/log/unknown)
  // stays unstyled so auto-detection can't mis-color it.
  const language = highlightLanguage(name);

  if (!language) {
    return '';
  }

  try {
    return hljs.highlight(code, { language, ignoreIllegals: true }).value;
  } catch {
    return '';
  }
}

const PdfPreview = defineAsyncComponent(() => import('./PdfPreview.vue'));

const quickLook = useQuickLook();
const store = useFileManagerStore();

// Resolve light vs dark so the code block matches the app appearance.
const systemPrefersLight = ref(
  typeof window !== 'undefined'
  && typeof window.matchMedia === 'function'
  && window.matchMedia('(prefers-color-scheme: light)').matches,
);
let appearanceQuery = null;
function onAppearanceChange(event) {
  systemPrefersLight.value = event.matches;
}
const isLightTheme = computed(() => {
  const mode = store.appSettings.appearanceMode;
  if (mode === 'light') {
    return true;
  }
  if (mode === 'dark') {
    return false;
  }
  return systemPrefersLight.value;
});

const mediaUrl = ref('');
const imageUrl = ref('');
const imageLoading = ref(false);
const textContent = ref('');
const textTruncated = ref(false);
const textError = ref('');
const textLoading = ref(false);
let loadVersion = 0;

// Matches PreviewPanel: cap how much a single preview will pull (relevant for
// remote images fetched over the network).
const IMAGE_PREVIEW_MAX_BYTES = 128 * 1024 * 1024;

const entry = computed(() => quickLook.current.value);
const isDirectory = computed(() => entry.value?.kind === 'directory');
const isLocalFile = computed(() => entry.value && !isArchivePath(entry.value.path));

const isImage = computed(() => Boolean(entry.value) && isImageEntry(entry.value) && isLocalFile.value);
const isVideo = computed(() => Boolean(entry.value) && isVideoEntry(entry.value) && isLocalFile.value);
const isAudio = computed(() => Boolean(entry.value) && isAudioEntry(entry.value) && isLocalFile.value);
const isPdf = computed(() => Boolean(entry.value) && isPdfEntry(entry.value) && isLocalFile.value);
const isText = computed(() => Boolean(entry.value) && isTextEntry(entry.value) && isLocalFile.value);
const isMediaLoading = computed(() => (
  (isImage.value && imageLoading.value && !imageUrl.value)
  || ((isVideo.value || isAudio.value) && !mediaUrl.value)
));
const highlightedCode = computed(() => (
  isText.value && textContent.value ? highlightCode(textContent.value, entry.value?.name) : ''
));

const positionLabel = computed(() => `${quickLook.index.value + 1} of ${quickLook.count.value}`);

function fmtSize(bytes) {
  const value = Number(bytes);

  if (!Number.isFinite(value)) {
    return '';
  }

  if (value >= 1024 ** 3) {
    return `${(value / 1024 ** 3).toFixed(1)} GB`;
  }

  if (value >= 1024 ** 2) {
    return `${(value / 1024 ** 2).toFixed(1)} MB`;
  }

  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KB`;
  }

  return `${value} B`;
}

const metaLine = computed(() => {
  const item = entry.value;

  if (!item) {
    return '';
  }

  const parts = [item.kind === 'directory' ? 'Folder' : 'File'];
  const size = item.kind === 'directory' ? '' : fmtSize(item.size);
  const date = item.modifiedAt
    ? formatFileDateTime(item.modifiedAt, store.appSettings.dateFormat, { fallback: '' })
    : '';

  if (size) {
    parts.push(size);
  }

  if (date) {
    parts.push(date);
  }

  return parts.join(' · ');
});

function mediaPayloadToBytes(payload) {
  if (payload instanceof ArrayBuffer || ArrayBuffer.isView(payload)) {
    return payload;
  }

  if (Array.isArray(payload)) {
    return new Uint8Array(payload);
  }

  throw new Error('Unexpected media preview payload.');
}

function imageMimeType(name) {
  const extension = extensionForName(name);

  if (extension === 'jpg' || extension === 'jpeg' || extension === 'jfif') return 'image/jpeg';
  if (extension === 'png') return 'image/png';
  if (extension === 'gif') return 'image/gif';
  if (extension === 'webp') return 'image/webp';
  if (extension === 'svg') return 'image/svg+xml';
  if (extension === 'bmp') return 'image/bmp';
  if (extension === 'avif') return 'image/avif';
  if (extension === 'ico') return 'image/x-icon';
  if (extension === 'tif' || extension === 'tiff') return 'image/tiff';
  return '';
}

function revokeImageUrl() {
  if (imageUrl.value.startsWith('blob:')) {
    URL.revokeObjectURL(imageUrl.value);
  }

  imageUrl.value = '';
}

async function loadPreview() {
  const token = ++loadVersion;
  const item = entry.value;

  mediaUrl.value = '';
  revokeImageUrl();
  imageLoading.value = false;
  textContent.value = '';
  textTruncated.value = false;
  textError.value = '';
  textLoading.value = false;

  if (!item) {
    return;
  }

  if (isImageEntry(item) && isLocalFile.value) {
    // Local files load straight from the asset protocol; remote files have no
    // asset URL, so pull the bytes and render them from an object URL.
    if (!isRemotePath(item.path)) {
      imageUrl.value = localFileAssetUrl(item.path);
      return;
    }

    if (typeof item.size === 'number' && item.size > IMAGE_PREVIEW_MAX_BYTES) {
      return;
    }

    imageLoading.value = true;

    try {
      const payload = await readMediaPreview(item.path, IMAGE_PREVIEW_MAX_BYTES);
      if (token === loadVersion) {
        const blob = new Blob([mediaPayloadToBytes(payload)], {
          type: imageMimeType(item.name) || 'application/octet-stream',
        });
        imageUrl.value = URL.createObjectURL(blob);
      }
    } catch {
      // Leave imageUrl empty so the generic icon view is shown.
    } finally {
      if (token === loadVersion) {
        imageLoading.value = false;
      }
    }
    return;
  }

  if (isVideoEntry(item) || isAudioEntry(item)) {
    if (!isLocalFile.value) {
      return;
    }

    try {
      const url = await createMediaStreamUrl(item.path);
      if (token === loadVersion) {
        mediaUrl.value = url || localFileAssetUrl(item.path);
      }
    } catch {
      if (token === loadVersion) {
        mediaUrl.value = localFileAssetUrl(item.path);
      }
    }
    return;
  }

  if (isTextEntry(item) && isLocalFile.value) {
    textLoading.value = true;

    try {
      const preview = await readTextPreview(item.path, 256 * 1024);
      if (token === loadVersion) {
        textContent.value = preview?.text || '';
        textTruncated.value = Boolean(preview?.truncated);
      }
    } catch (error) {
      if (token === loadVersion) {
        textError.value = error?.message || 'Unable to load preview.';
      }
    } finally {
      if (token === loadVersion) {
        textLoading.value = false;
      }
    }
  }
}

watch(() => entry.value?.path, loadPreview, { immediate: true });

function close() {
  quickLook.close();
}

function onKeydown(event) {
  switch (event.key) {
    case ' ':
    case 'Escape':
      event.preventDefault();
      event.stopPropagation();
      close();
      break;
    case 'ArrowLeft':
    case 'ArrowUp':
      event.preventDefault();
      event.stopPropagation();
      quickLook.prev();
      break;
    case 'ArrowRight':
    case 'ArrowDown':
      event.preventDefault();
      event.stopPropagation();
      quickLook.next();
      break;
    default:
      break;
  }
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown, true);

  if (typeof window !== 'undefined' && typeof window.matchMedia === 'function') {
    appearanceQuery = window.matchMedia('(prefers-color-scheme: light)');
    systemPrefersLight.value = appearanceQuery.matches;
    appearanceQuery.addEventListener?.('change', onAppearanceChange);
  }
});

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown, true);
  appearanceQuery?.removeEventListener?.('change', onAppearanceChange);
  revokeImageUrl();
});
</script>

<template>
  <Teleport to="body">
    <Transition name="quicklook-fade">
      <div
        v-if="quickLook.visible.value"
        class="quicklook-overlay"
        role="dialog"
        aria-modal="true"
        :aria-label="`Quick Look: ${entry?.name || ''}`"
        @pointerdown.self="close"
      >
        <div class="quicklook-panel">
          <header class="quicklook-header">
            <div class="quicklook-title">
              <AppIcon :name="isDirectory ? 'folder' : fileTypeIconName(entry || {})" :size="16" :stroke-width="1.7" />
              <span class="quicklook-name" :title="entry?.path">{{ entry?.name }}</span>
            </div>
            <div class="quicklook-header-right">
              <span v-if="quickLook.count.value > 1" class="quicklook-count">{{ positionLabel }}</span>
              <button type="button" class="quicklook-close" aria-label="Close Quick Look" @click="close">
                <AppIcon name="x" :size="14" :stroke-width="2" />
              </button>
            </div>
          </header>

          <div class="quicklook-body">
            <div class="quicklook-stage">
              <img v-if="isImage && imageUrl" class="quicklook-image" :src="imageUrl" :alt="entry?.name" />

              <video
                v-else-if="isVideo && mediaUrl"
                :key="mediaUrl"
                class="quicklook-media"
                :src="mediaUrl"
                controls
                playsinline
              ></video>

              <div v-else-if="isAudio && mediaUrl" class="quicklook-audio">
                <AppIcon name="music" :size="64" :stroke-width="1.4" />
                <audio :key="mediaUrl" class="quicklook-audio-player" :src="mediaUrl" controls></audio>
              </div>

              <PdfPreview v-else-if="isPdf" class="quicklook-pdf" :entry="entry" />

              <div v-else-if="isText" class="quicklook-text-shell" :class="{ 'quicklook-text-shell--light': isLightTheme }">
                <div v-if="textLoading" class="quicklook-status">Loading preview…</div>
                <div v-else-if="textError" class="quicklook-status">{{ textError }}</div>
                <template v-else>
                  <pre class="quicklook-code"><code v-if="highlightedCode" class="hljs" v-html="highlightedCode"></code><code v-else class="hljs">{{ textContent }}</code></pre>
                  <span v-if="textTruncated" class="quicklook-truncated">Preview truncated</span>
                </template>
              </div>

              <div v-else-if="isMediaLoading" class="quicklook-status">Loading preview…</div>

              <div v-else class="quicklook-generic">
                <AppIcon :name="isDirectory ? 'folder' : fileTypeIconName(entry || {})" :size="92" :stroke-width="1.2" />
                <span class="quicklook-generic-name">{{ entry?.name }}</span>
                <span class="quicklook-generic-meta">{{ metaLine }}</span>
              </div>
            </div>
          </div>

          <footer class="quicklook-footer">
            <span class="quicklook-meta">{{ metaLine }}</span>
            <span class="quicklook-hint">Space to close · ← → to browse</span>
          </footer>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.quicklook-overlay {
  position: fixed;
  z-index: 5200;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 40px;
  background: color-mix(in srgb, var(--overlay-bg) 80%, transparent);
  -webkit-backdrop-filter: blur(16px) saturate(1.1);
  backdrop-filter: blur(16px) saturate(1.1);
}

.quicklook-panel {
  display: flex;
  flex-direction: column;
  width: min(1100px, 90vw);
  height: min(820px, 88vh);
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  background: var(--modal-bg);
  box-shadow: var(--shadow-overlay);
}

.quicklook-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-shrink: 0;
  padding: 11px 12px 11px 16px;
  border-bottom: 1px solid var(--hairline);
}

.quicklook-title {
  display: flex;
  align-items: center;
  gap: 9px;
  min-width: 0;
  color: var(--text);
}

.quicklook-name {
  overflow: hidden;
  font-size: 13px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.quicklook-header-right {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.quicklook-count {
  color: var(--text-faint);
  font-size: 12px;
  font-weight: 600;
}

.quicklook-close {
  display: grid;
  width: 26px;
  height: 26px;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.quicklook-close:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.quicklook-body {
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
  padding: 14px;
}

.quicklook-stage {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: 1 1 auto;
  min-width: 0;
  height: 100%;
  overflow: hidden;
}

.quicklook-image {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
  border-radius: 6px;
}

.quicklook-media {
  max-width: 100%;
  max-height: 100%;
  border-radius: 6px;
  background: #000;
}

.quicklook-audio {
  display: grid;
  justify-items: center;
  gap: 22px;
  color: var(--icon);
}

.quicklook-audio-player {
  width: min(440px, 70vw);
}

.quicklook-pdf {
  width: 100%;
  height: 100%;
}

.quicklook-text-shell {
  /* One Dark syntax palette (default). */
  --ql-bg: #282c34;
  --ql-fg: #abb2bf;
  --ql-comment: #5c6370;
  --ql-keyword: #c678dd;
  --ql-name: #e06c75;
  --ql-literal: #56b6c2;
  --ql-string: #98c379;
  --ql-attr: #d19a66;
  --ql-title: #61aeee;
  --ql-builtin: #e6c07b;
  position: relative;
  width: 100%;
  height: 100%;
  overflow: auto;
  border-radius: 9px;
  border: 1px solid var(--hairline);
  background: var(--ql-bg);
}

.quicklook-text-shell--light {
  /* One Light syntax palette. */
  --ql-bg: #fafafa;
  --ql-fg: #383a42;
  --ql-comment: #a0a1a7;
  --ql-keyword: #a626a4;
  --ql-name: #e45649;
  --ql-literal: #0184bb;
  --ql-string: #50a14f;
  --ql-attr: #986801;
  --ql-title: #4078f2;
  --ql-builtin: #c18401;
}

.quicklook-code {
  margin: 0;
}

.quicklook-code code {
  display: block;
  padding: 16px 18px;
  color: var(--ql-fg);
  font-family: ui-monospace, SFMono-Regular, Menlo, "Cascadia Code", monospace;
  font-size: 12.5px;
  line-height: 1.6;
  tab-size: 4;
  white-space: pre;
}

:deep(.hljs-comment),
:deep(.hljs-quote) {
  color: var(--ql-comment);
  font-style: italic;
}

:deep(.hljs-doctag),
:deep(.hljs-keyword),
:deep(.hljs-formula) {
  color: var(--ql-keyword);
}

:deep(.hljs-section),
:deep(.hljs-name),
:deep(.hljs-selector-tag),
:deep(.hljs-deletion),
:deep(.hljs-subst) {
  color: var(--ql-name);
}

:deep(.hljs-literal) {
  color: var(--ql-literal);
}

:deep(.hljs-string),
:deep(.hljs-regexp),
:deep(.hljs-addition),
:deep(.hljs-attribute),
:deep(.hljs-meta .hljs-string) {
  color: var(--ql-string);
}

:deep(.hljs-attr),
:deep(.hljs-variable),
:deep(.hljs-template-variable),
:deep(.hljs-type),
:deep(.hljs-selector-class),
:deep(.hljs-selector-attr),
:deep(.hljs-selector-pseudo),
:deep(.hljs-number) {
  color: var(--ql-attr);
}

:deep(.hljs-symbol),
:deep(.hljs-bullet),
:deep(.hljs-link),
:deep(.hljs-meta),
:deep(.hljs-selector-id),
:deep(.hljs-title) {
  color: var(--ql-title);
}

:deep(.hljs-built_in),
:deep(.hljs-title.class_),
:deep(.hljs-class .hljs-title) {
  color: var(--ql-builtin);
}

:deep(.hljs-emphasis) {
  font-style: italic;
}

:deep(.hljs-strong) {
  font-weight: 700;
}

:deep(.hljs-link) {
  text-decoration: underline;
}

.quicklook-truncated {
  position: sticky;
  bottom: 0;
  display: block;
  padding: 6px 12px;
  background: color-mix(in srgb, var(--ql-bg) 92%, transparent);
  color: color-mix(in srgb, var(--ql-fg) 55%, transparent);
  font-size: 11px;
  text-align: center;
}

.quicklook-status {
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 540;
}

.quicklook-generic {
  display: grid;
  justify-items: center;
  gap: 14px;
  color: var(--icon);
  text-align: center;
  padding: 24px;
}

.quicklook-generic-name {
  max-width: 60ch;
  overflow-wrap: anywhere;
  color: var(--text);
  font-size: 15px;
  font-weight: 650;
}

.quicklook-generic-meta {
  color: var(--text-faint);
  font-size: 12px;
}

.quicklook-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-shrink: 0;
  padding: 9px 16px;
  border-top: 1px solid var(--hairline);
}

.quicklook-meta {
  color: var(--text-muted);
  font-size: 12px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.quicklook-hint {
  flex-shrink: 0;
  color: var(--text-faint);
  font-size: 11px;
}

.quicklook-fade-enter-active {
  transition: opacity 160ms ease;
}
.quicklook-fade-leave-active {
  transition: opacity 130ms ease;
}
.quicklook-fade-enter-active .quicklook-panel {
  transition: transform 200ms cubic-bezier(0.2, 0, 0, 1), opacity 160ms ease;
}
.quicklook-fade-leave-active .quicklook-panel {
  transition: transform 120ms ease, opacity 110ms ease;
}
.quicklook-fade-enter-from,
.quicklook-fade-leave-to {
  opacity: 0;
}
.quicklook-fade-enter-from .quicklook-panel,
.quicklook-fade-leave-to .quicklook-panel {
  opacity: 0;
  transform: scale(0.98);
}
</style>
