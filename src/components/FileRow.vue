<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { canUseLocalFileAssets, localFileAssetUrl } from '../composables/useFileOperations';
import { isArchiveEntry } from '../utils/archivePaths';
import { formatFileDate } from '../utils/dateFormat';
import { fileTypeIconKind, fileTypeIconName } from '../utils/fileTypeIcons';
import { isImageEntry } from '../utils/fileTypes';

const props = defineProps({
  entry: {
    type: Object,
    required: true,
  },
  selected: {
    type: Boolean,
    required: true,
  },
  variant: {
    type: String,
    default: 'list',
  },
  dateFormat: {
    type: String,
    default: 'system',
  },
  striped: {
    type: Boolean,
    default: false,
  },
  draggable: {
    type: Boolean,
    default: false,
  },
});

const emit = defineEmits([
  'open',
  'drag-start',
  'drag-end',
  'drag-over',
  'drag-leave',
  'drop',
]);

const cardElement = ref(null);
const imageLoaded = ref(false);
const imageFailed = ref(false);
const shouldLoadThumbnail = ref(false);
let thumbnailObserver = null;

const isGridImage = computed(() => props.variant === 'grid' && shouldShowImage(props.entry));
const thumbnailSrc = computed(() => {
  if (!isGridImage.value || !shouldLoadThumbnail.value || imageFailed.value) {
    return '';
  }

  return localFileAssetUrl(props.entry.path);
});

function formatSize(size) {
  if (size === null || size === undefined) {
    return '';
  }

  if (size < 1024) {
    return `${size} bytes`;
  }

  if (size < 1024 * 1024) {
    return `${(size / 1024).toFixed(0)} KB`;
  }

  return `${(size / (1024 * 1024)).toFixed(1).replace('.', ',')} MB`;
}

function shouldShowImage(entry) {
  return isImageEntry(entry);
}

function visualThumbClass(entry) {
  const slug = String(entry.name || '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');

  return slug ? `visual-thumb--${slug}` : '';
}

function fileTypeClass(entry, prefix) {
  return `${prefix}--${fileTypeIconKind(entry)}`;
}

function stopThumbnailObserver() {
  thumbnailObserver?.disconnect();
  thumbnailObserver = null;
}

function observeThumbnail() {
  stopThumbnailObserver();

  if (!isGridImage.value || !canUseLocalFileAssets()) {
    return;
  }

  const element = cardElement.value;

  if (!element) {
    return;
  }

  if (typeof IntersectionObserver === 'undefined') {
    shouldLoadThumbnail.value = true;
    return;
  }

  thumbnailObserver = new IntersectionObserver(
    (entries) => {
      if (entries.some((entry) => entry.isIntersecting || entry.intersectionRatio > 0)) {
        shouldLoadThumbnail.value = true;
        stopThumbnailObserver();
      }
    },
    { root: null, rootMargin: '360px 0px', threshold: 0.01 },
  );
  thumbnailObserver.observe(element);
}

function resetThumbnail() {
  imageLoaded.value = false;
  imageFailed.value = false;
  shouldLoadThumbnail.value = false;
  nextTick(observeThumbnail);
}

function handleImageLoad() {
  imageLoaded.value = true;
}

function handleImageError() {
  imageFailed.value = true;
  imageLoaded.value = false;
}

watch(() => [props.entry.path, props.variant], resetThumbnail);

onMounted(() => {
  nextTick(observeThumbnail);
});

onBeforeUnmount(stopThumbnailObserver);
</script>

<template>
  <button
    v-if="variant === 'grid'"
    ref="cardElement"
    type="button"
    class="file-card"
    :class="{
      'file-card--selected': selected,
      'file-card--directory': entry.kind === 'directory',
    }"
    :draggable="draggable"
    @dblclick="emit('open')"
    @dragstart.stop="emit('drag-start', $event)"
    @dragend.stop="emit('drag-end', $event)"
    @dragover="emit('drag-over', $event)"
    @dragleave="emit('drag-leave', $event)"
    @drop="emit('drop', $event)"
  >
    <span
      class="file-card-frame"
      :class="{
        'file-card-frame--icon': entry.kind === 'directory',
        'file-card-frame--archive': isArchiveEntry(entry),
        'file-card-frame--file': entry.kind !== 'directory' && !shouldShowImage(entry),
        'file-card-frame--photo': entry.kind !== 'directory' && shouldShowImage(entry),
        [fileTypeClass(entry, 'file-card-frame')]: entry.kind !== 'directory' && !shouldShowImage(entry),
      }"
    >
      <AppIcon v-if="entry.kind === 'directory'" name="folder" :size="58" :stroke-width="1.55" />
      <span
        v-else-if="shouldShowImage(entry)"
        class="file-card-image-shell"
        :class="{ 'file-card-image-shell--loaded': imageLoaded }"
      >
        <span
          class="file-card-image file-card-image--placeholder visual-thumb"
          :class="visualThumbClass(entry)"
          aria-hidden="true"
        ></span>
        <img
          v-if="thumbnailSrc"
          class="file-card-image file-card-image--real"
          :class="{ 'file-card-image--loaded': imageLoaded }"
          :src="thumbnailSrc"
          :alt="entry.name"
          loading="lazy"
          decoding="async"
          draggable="false"
          @load="handleImageLoad"
          @error="handleImageError"
        />
      </span>
      <AppIcon v-else :name="fileTypeIconName(entry)" :size="46" :stroke-width="1.55" />
    </span>
    <span class="file-card-name">{{ entry.name }}</span>
    <span
      v-if="entry.tagColor"
      class="file-card-tag"
      :style="{ '--tag-color': entry.tagColor }"
      aria-hidden="true"
    ></span>
  </button>

  <button
    v-else
    type="button"
    class="file-row"
    :class="{
      'file-row--striped': striped,
      'file-row--selected': selected,
      'file-row--directory': entry.kind === 'directory',
      'file-row--archive': isArchiveEntry(entry),
    }"
    :draggable="draggable"
    @dblclick="emit('open')"
    @dragstart.stop="emit('drag-start', $event)"
    @dragend.stop="emit('drag-end', $event)"
    @dragover="emit('drag-over', $event)"
    @dragleave="emit('drag-leave', $event)"
    @drop="emit('drop', $event)"
  >
    <span class="file-name">
      <span class="file-glyph" :class="[ `file-glyph--${entry.kind}`, fileTypeClass(entry, 'file-glyph') ]">
        <AppIcon
          :name="fileTypeIconName(entry)"
          :size="18"
          :stroke-width="1.8"
        />
      </span>
      <span>{{ entry.name }}</span>
    </span>
    <span class="tag-cell">
      <span v-if="entry.tagColor" class="tag-dot" :style="{ '--tag-color': entry.tagColor }" aria-hidden="true"></span>
    </span>
    <span class="muted">{{ formatSize(entry.size) }}</span>
    <span class="muted">{{ formatFileDate(entry.modifiedAt, dateFormat) }}</span>
  </button>
</template>

<style scoped>
/* ── List row ─────────────────────────────────────────────── */
.file-row {
  display: grid;
  width: 100%;
  grid-template-columns: var(--file-list-grid-columns, minmax(180px, 1fr) 46px 88px 126px);
  align-items: center;
  gap: 12px;
  height: 29px;
  min-height: 0;
  border-radius: 0;
  padding: 2px 20px 2px 34px;
  background: transparent;
  color: var(--text);
  text-align: left;
  font-size: 14px;
  transition: background 80ms ease;
}

.file-row--striped {
  background: color-mix(in srgb, var(--text) 3.2%, transparent);
}

.file-row:hover {
  background: var(--btn-hover);
}

.file-row--selected {
  background: var(--btn-primary-bg);
  color: white;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.16);
}

.file-row--selected:hover {
  background: var(--btn-primary-bg-hover);
}

.file-row--selected .muted,
.file-row--selected .file-glyph {
  color: rgb(255 255 255 / 0.88);
}

/* ── File name cell ───────────────────────────────────────── */
.file-name {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 7px;
  padding-right: 6px;
  font-weight: 400;
}

.file-row--directory .file-name {
  font-weight: 700;
}

.file-name span:last-child {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-glyph {
  display: grid;
  width: 18px;
  height: 18px;
  flex: 0 0 auto;
  place-items: center;
  color: var(--file-icon);
}

.file-glyph--directory {
  color: var(--folder-icon);
}

.file-glyph--archive,
.file-glyph--audio,
.file-glyph--code,
.file-glyph--config,
.file-glyph--document,
.file-glyph--image,
.file-glyph--spreadsheet,
.file-glyph--presentation,
.file-glyph--video {
  color: color-mix(in srgb, var(--file-icon) 82%, var(--file-type-tint, var(--accent)) 18%);
}

.file-glyph--archive,
.file-glyph--spreadsheet {
  --file-type-tint: var(--folder-icon);
}

.file-glyph--audio,
.file-glyph--config,
.file-glyph--presentation,
.file-glyph--video {
  --file-type-tint: var(--accent-warm);
}

.file-glyph--code,
.file-glyph--document,
.file-glyph--image {
  --file-type-tint: var(--accent);
}

.file-row--selected .file-glyph {
  color: rgb(255 255 255 / 0.88);
}

/* ── Tag cell ─────────────────────────────────────────────── */
.tag-cell {
  display: flex;
  justify-content: center;
}

.tag-dot {
  width: 11px;
  height: 11px;
  border-radius: 50%;
  background: var(--tag-color);
  box-shadow:
    inset 0 0 0 0.5px rgb(255 255 255 / 0.35),
    0 1px 2px rgb(0 0 0 / 0.22);
}

.muted {
  overflow: hidden;
  padding-right: 8px;
  padding-left: 3px;
  color: var(--text-muted);
  font-size: 14px;
  font-weight: 400;
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ── Grid card ────────────────────────────────────────────── */
.file-card {
  position: relative;
  display: grid;
  width: 194px;
  min-width: 0;
  justify-self: center;
  justify-items: center;
  gap: 14px;
  border-radius: 6px;
  padding: 12px 10px 10px;
  background: transparent;
  color: var(--text);
  text-align: center;
  transition: background 100ms ease;
}

.file-card-tag {
  position: absolute;
  top: 10px;
  right: 12px;
  width: 12px;
  height: 12px;
  border-radius: 50%;
  background: var(--tag-color);
  box-shadow:
    inset 0 0 0 0.5px rgb(255 255 255 / 0.4),
    0 1px 3px rgb(0 0 0 / 0.35);
}

.file-card:hover {
  background: var(--btn-hover);
}

.file-card-frame {
  display: grid;
  width: 166px;
  height: 118px;
  place-items: center;
  border-radius: 0;
  transition: box-shadow 150ms ease;
}

.file-card-frame--icon {
  color: var(--folder-icon);
  background: color-mix(in srgb, var(--folder-icon) 8%, transparent);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--folder-icon) 14%, transparent);
}

.file-card-frame--file {
  color: var(--file-icon);
}

.file-card-frame--archive,
.file-card-frame--audio,
.file-card-frame--code,
.file-card-frame--config,
.file-card-frame--document,
.file-card-frame--image,
.file-card-frame--spreadsheet,
.file-card-frame--presentation,
.file-card-frame--video {
  color: color-mix(in srgb, var(--file-icon) 82%, var(--file-type-tint, var(--accent)) 18%);
}

.file-card-frame--archive,
.file-card-frame--spreadsheet {
  --file-type-tint: var(--folder-icon);
}

.file-card-frame--audio,
.file-card-frame--config,
.file-card-frame--presentation,
.file-card-frame--video {
  --file-type-tint: var(--accent-warm);
}

.file-card-frame--code,
.file-card-frame--document,
.file-card-frame--image {
  --file-type-tint: var(--accent);
}

.file-card-frame--photo {
  background: transparent;
}

.file-card-image-shell {
  display: grid;
  width: 100%;
  height: 100%;
  border-radius: 0;
  overflow: hidden;
}

.file-card-image {
  display: block;
  width: 100%;
  height: 100%;
  grid-area: 1 / 1;
}

.file-card-image--placeholder {
  transition: opacity 120ms ease;
}

.file-card-image--real {
  width: auto;
  height: auto;
  max-width: 100%;
  max-height: 100%;
  place-self: center;
  border: 1px solid var(--hairline);
  background: var(--control-glass);
  box-shadow:
    0 1px 1px rgb(255 255 255 / 0.14),
    0 8px 13px color-mix(in srgb, var(--text) 18%, transparent);
  object-fit: contain;
  opacity: 0;
  transition: opacity 140ms ease;
}

.file-card-image--loaded {
  opacity: 1;
}

.file-card-image-shell--loaded .file-card-image--placeholder {
  opacity: 0;
}

.file-card--selected {
  background: rgb(var(--accent-rgb) / 0.12);
}

.file-card--selected:hover {
  background: rgb(var(--accent-rgb) / 0.18);
}

/* Selected card */
.file-card--selected .file-card-frame--icon {
  background:
    linear-gradient(180deg, rgb(var(--accent-rgb) / 0.2), rgb(var(--accent-rgb) / 0.08)),
    rgb(var(--accent-rgb) / 0.1);
  box-shadow:
    inset 0 0 0 1.5px var(--accent-border),
    inset 0 1px 0 rgb(255 255 255 / 0.14),
    0 8px 24px rgb(var(--accent-rgb) / 0.2);
}

.file-card-name {
  max-width: 160px;
  overflow: hidden;
  border-radius: 5px;
  padding: 2px 7px 3px;
  font-size: 14px;
  font-weight: 400;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-card--directory .file-card-name {
  font-weight: 700;
}

.file-card--selected .file-card-name {
  background: var(--btn-primary-bg);
  color: white;
  box-shadow: 0 2px 8px rgb(var(--accent-rgb) / 0.3);
}

.file-card--selected:hover .file-card-name {
  background: var(--btn-primary-bg-hover);
}
</style>
