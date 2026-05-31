<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { useScrollableContentState } from '../composables/useScrollableContentState';
import { extensionForName } from '../utils/fileTypes';

const DESKTOP_PREVIEW_ROW_HEIGHT = 34;
const MOBILE_PREVIEW_ROW_HEIGHT = 72;
const PREVIEW_OVERSCAN_ROWS = 12;

const FORMATS = [
  { value: 'webp', label: 'WebP', extension: 'webp', icon: 'image' },
  { value: 'avif', label: 'AVIF', extension: 'avif', icon: 'image' },
  { value: 'png', label: 'PNG', extension: 'png', icon: 'image' },
  { value: 'jpeg', label: 'JPEG', extension: 'jpg', icon: 'image' },
  { value: 'tiff', label: 'TIFF', extension: 'tiff', icon: 'image' },
  { value: 'bmp', label: 'BMP', extension: 'bmp', icon: 'image' },
  { value: 'ico', label: 'ICO', extension: 'ico', icon: 'image' },
];

const CONFLICT_POLICIES = [
  { value: 'keepBoth', label: 'Keep Both', icon: 'copy' },
  { value: 'skip', label: 'Skip', icon: 'minus' },
  { value: 'replace', label: 'Replace', icon: 'refresh' },
];

const props = defineProps({
  visible: {
    type: Boolean,
    default: false,
  },
  entries: {
    type: Array,
    default: () => [],
  },
  existingNamesByDirectory: {
    type: Object,
    default: () => ({}),
  },
  otherPaneDirectory: {
    type: String,
    default: '',
  },
});

const emit = defineEmits(['cancel', 'convert']);

const panelRef = ref(null);
const controlsRef = ref(null);
const previewListRef = ref(null);
const previewScrollTop = ref(0);
const previewViewportHeight = ref(0);
const previewRowHeight = ref(DESKTOP_PREVIEW_ROW_HEIGHT);
const targetFormat = ref('webp');
const conflictPolicy = ref('keepBoth');
const destinationMode = ref('sameFolder');
const imageQuality = ref(85);
let previewResizeObserver = null;
const { isScrollable: controlsScrollable } = useScrollableContentState(controlsRef, {
  watch: [
    () => props.visible,
    targetFormat,
    () => props.otherPaneDirectory,
  ],
});
const { isScrollable: previewScrollable } = useScrollableContentState(previewListRef, {
  watch: [
    () => props.visible,
    () => props.entries.length,
    targetFormat,
    conflictPolicy,
  ],
});
const imageConvertContentScrollable = computed(() => (
  controlsScrollable.value || previewScrollable.value
));

const currentFormat = computed(() => (
  FORMATS.find((format) => format.value === targetFormat.value) || FORMATS[0]
));
const selectedLabel = computed(() => {
  const count = props.entries.length;
  return `${count} ${count === 1 ? 'image' : 'images'} selected`;
});
const canUseOtherPane = computed(() => Boolean(props.otherPaneDirectory));
const sourceDirectory = computed(() => {
  const directories = new Set(
    props.entries.map((entry) => entry?.directory || parentDirectoryForPath(entry?.path)),
  );

  return directories.size === 1 ? [...directories][0] : '';
});
const destinationLabel = computed(() => {
  const target = destinationMode.value === 'otherPane' && props.otherPaneDirectory
    ? props.otherPaneDirectory
    : sourceDirectory.value;

  return target ? directoryName(target) : 'Same folder';
});
const previewRows = computed(() => {
  const plannedByDirectory = new Map();

  return props.entries.map((entry, index) => {
    const directory = outputDirectoryFor(entry);
    const seedName = outputNameFor(entry?.name || `Image ${index + 1}`, currentFormat.value.extension);
    const existingNames = existingNameSet(directory);
    const plannedNames = plannedByDirectory.get(directory) || new Set();
    plannedByDirectory.set(directory, plannedNames);

    const lowerSeed = seedName.toLocaleLowerCase();
    const exists = existingNames.has(lowerSeed);
    const planned = plannedNames.has(lowerSeed);
    let nextName = seedName;
    let status = 'ready';
    let message = 'Ready';

    if (planned) {
      nextName = uniqueOutputName(seedName, (candidate) =>
        existingNames.has(candidate.toLocaleLowerCase()) || plannedNames.has(candidate.toLocaleLowerCase()),
      );
      message = 'Renamed';
    } else if (exists) {
      if (conflictPolicy.value === 'skip') {
        status = 'skip';
        message = 'Exists';
      } else if (conflictPolicy.value === 'replace') {
        status = 'replace';
        message = 'Replace';
      } else {
        nextName = uniqueOutputName(seedName, (candidate) =>
          existingNames.has(candidate.toLocaleLowerCase()) || plannedNames.has(candidate.toLocaleLowerCase()),
        );
        message = 'Renamed';
      }
    }

    if (status !== 'skip') {
      plannedNames.add(nextName.toLocaleLowerCase());
    }

    return {
      id: entry?.path || `${entry?.name || 'image'}-${index}`,
      entry,
      index,
      directory,
      nextName,
      status,
      message,
    };
  });
});
const previewSummary = computed(() => {
  let ready = 0;
  let replace = 0;
  let skip = 0;

  for (const row of previewRows.value) {
    if (row.status === 'replace') {
      replace += 1;
    } else if (row.status === 'skip') {
      skip += 1;
    } else {
      ready += 1;
    }
  }

  return { ready, replace, skip };
});
const conversionCount = computed(() => previewSummary.value.ready + previewSummary.value.replace);
const canConvert = computed(() => conversionCount.value > 0);
const primaryLabel = computed(() => (
  conversionCount.value === 1 ? 'Convert 1 Image' : `Convert ${conversionCount.value} Images`
));
const summaryText = computed(() => {
  if (conversionCount.value > 0) {
    return `${conversionCount.value} ${conversionCount.value === 1 ? 'output' : 'outputs'}`;
  }

  return 'No outputs';
});
const summaryTone = computed(() => (conversionCount.value > 0 ? 'ready' : 'idle'));
const qualityFormatLabel = computed(() => (
  currentFormat.value.label
));
const supportsQuality = computed(() => ['avif', 'jpeg', 'webp'].includes(targetFormat.value));
const conversionHint = computed(() => `Save each image as ${currentFormat.value.label}. The originals are kept.`);
const virtualPreviewRange = computed(() => {
  const rowHeight = Math.max(1, previewRowHeight.value);
  const count = previewRows.value.length;
  const viewportHeight = Math.max(previewViewportHeight.value || rowHeight * 10, rowHeight);
  const start = Math.min(
    Math.max(0, count - 1),
    Math.max(0, Math.floor(previewScrollTop.value / rowHeight) - PREVIEW_OVERSCAN_ROWS),
  );
  const visibleCount = Math.ceil(viewportHeight / rowHeight) + (PREVIEW_OVERSCAN_ROWS * 2);
  const end = Math.min(count, start + visibleCount);

  return {
    start,
    end,
    paddingBefore: start * rowHeight,
    paddingAfter: Math.max(0, (count - end) * rowHeight),
  };
});
const virtualPreviewRows = computed(() => (
  previewRows.value
    .slice(virtualPreviewRange.value.start, virtualPreviewRange.value.end)
    .map((row, offset) => ({
      ...row,
      virtualIndex: virtualPreviewRange.value.start + offset,
    }))
));
const previewRowStyle = computed(() => ({
  '--image-convert-row-height': `${previewRowHeight.value}px`,
}));

function resetState() {
  targetFormat.value = 'webp';
  conflictPolicy.value = 'keepBoth';
  destinationMode.value = 'sameFolder';
  imageQuality.value = 85;
}

function updatePreviewMetrics() {
  previewViewportHeight.value = previewListRef.value?.clientHeight || 0;
  previewRowHeight.value = typeof window !== 'undefined' && window.matchMedia?.('(max-width: 620px)').matches
    ? MOBILE_PREVIEW_ROW_HEIGHT
    : DESKTOP_PREVIEW_ROW_HEIGHT;
}

function observePreviewList() {
  nextTick(() => {
    previewResizeObserver?.disconnect?.();
    previewResizeObserver = null;
    updatePreviewMetrics();

    if (typeof ResizeObserver !== 'undefined' && previewListRef.value) {
      previewResizeObserver = new ResizeObserver(updatePreviewMetrics);
      previewResizeObserver.observe(previewListRef.value);
    }
  });
}

function handlePreviewScroll(event) {
  previewScrollTop.value = event.target?.scrollTop || 0;
}

function outputDirectoryFor(entry) {
  if (destinationMode.value === 'otherPane' && props.otherPaneDirectory) {
    return props.otherPaneDirectory;
  }

  return entry?.directory || parentDirectoryForPath(entry?.path);
}

function existingNameSet(directory) {
  return new Set(
    (props.existingNamesByDirectory[directory] || [])
      .map((name) => String(name || '').toLocaleLowerCase()),
  );
}

function outputNameFor(name, extension) {
  const stem = fileStemForName(name);
  const candidate = `${stem}.${extension}`;

  return String(name || '').toLocaleLowerCase() === candidate.toLocaleLowerCase()
    ? `${stem} converted.${extension}`
    : candidate;
}

function fileStemForName(name) {
  const cleanName = String(name || '')
    .replace(/[\\/]/g, ' ')
    .trim()
    .replace(/^\.+|\.+$/g, '');

  if (!cleanName) {
    return 'Image';
  }

  const extension = extensionForName(cleanName);

  if (!extension) {
    return cleanName;
  }

  return cleanName.slice(0, -(extension.length + 1)).trim() || cleanName;
}

function uniqueOutputName(seedName, exists) {
  if (!exists(seedName)) {
    return seedName;
  }

  const extension = extensionForName(seedName);
  const suffix = extension ? `.${extension}` : '';
  const stem = extension ? seedName.slice(0, -(extension.length + 1)) : seedName;

  for (let index = 2; index < 1000; index += 1) {
    const candidate = `${stem} ${index}${suffix}`;

    if (!exists(candidate)) {
      return candidate;
    }
  }

  return `${stem} copy${suffix}`;
}

function parentDirectoryForPath(path) {
  const value = String(path || '').replace(/\/+$/, '');

  if (!value || value === '/' || value === '~') {
    return value || '~';
  }

  if (value.startsWith('remote://')) {
    const rest = value.slice('remote://'.length);
    const slashIndex = rest.indexOf('/');
    const volumeId = slashIndex >= 0 ? rest.slice(0, slashIndex) : rest;
    const objectPath = slashIndex >= 0 ? rest.slice(slashIndex + 1).replace(/\/+$/, '') : '';

    if (!volumeId || !objectPath) {
      return `remote://${volumeId}/`;
    }

    const parentIndex = objectPath.lastIndexOf('/');
    return parentIndex < 0
      ? `remote://${volumeId}/`
      : `remote://${volumeId}/${objectPath.slice(0, parentIndex)}`;
  }

  const slashIndex = value.lastIndexOf('/');
  return slashIndex <= 0 ? '/' : value.slice(0, slashIndex);
}

function directoryName(path) {
  const value = String(path || '').replace(/\/+$/, '');

  if (!value || value === '/') {
    return 'Root';
  }

  if (value.startsWith('remote://')) {
    const parts = value.slice('remote://'.length).split('/').filter(Boolean);
    return parts.at(-1) || parts[0] || 'Remote';
  }

  return value.split('/').filter(Boolean).at(-1) || value;
}

function cancel() {
  emit('cancel');
}

function convert() {
  if (!canConvert.value) {
    return;
  }

  emit('convert', {
    paths: props.entries.map((entry) => entry.path),
    options: {
      format: targetFormat.value,
      quality: supportsQuality.value ? Number(imageQuality.value || 85) : null,
      conflict: conflictPolicy.value,
      destinationDirectory: destinationMode.value === 'otherPane' ? props.otherPaneDirectory : null,
    },
  });
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault();
    cancel();
    return;
  }

  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    convert();
  }
}

watch(() => props.visible, (visible) => {
  if (!visible) {
    return;
  }

  resetState();
  previewScrollTop.value = 0;
  nextTick(() => panelRef.value?.focus?.({ preventScroll: true }));
  observePreviewList();
});

watch([targetFormat, conflictPolicy, destinationMode], () => {
  previewScrollTop.value = 0;
});

onMounted(() => {
  observePreviewList();
  window.addEventListener('resize', updatePreviewMetrics);

  if (props.visible) {
    nextTick(() => panelRef.value?.focus?.({ preventScroll: true }));
  }
});

onBeforeUnmount(() => {
  previewResizeObserver?.disconnect?.();
  window.removeEventListener('resize', updatePreviewMetrics);
});
</script>

<template>
  <Teleport to="body">
    <Transition name="image-convert-dialog">
      <div
        v-if="visible"
        class="image-convert-overlay"
        role="presentation"
        @pointerdown.self="cancel"
        @keydown.stop="handleKeydown"
      >
        <section
          ref="panelRef"
          class="image-convert-panel"
          :class="{ 'image-convert-panel--content-scrollable': imageConvertContentScrollable }"
          role="dialog"
          aria-modal="true"
          aria-labelledby="image-convert-title"
          tabindex="-1"
        >
          <header class="image-convert-header">
            <div class="image-convert-title-row">
              <span class="image-convert-icon" aria-hidden="true">
                <AppIcon name="image" :size="20" :stroke-width="1.9" />
              </span>
              <div class="image-convert-title-copy">
                <h2 id="image-convert-title">Convert Images</h2>
                <p>{{ selectedLabel }}</p>
              </div>
            </div>

            <div class="image-convert-summary" :class="`image-convert-summary--${summaryTone}`">
              <strong>{{ summaryText }}</strong>
              <span>{{ previewSummary.skip }} skipped</span>
            </div>
          </header>

          <div class="image-convert-layout">
            <section ref="controlsRef" class="image-convert-controls" aria-label="Conversion options">
              <p class="image-convert-hint">{{ conversionHint }}</p>

              <div class="image-convert-section">
                <span class="image-convert-eyebrow">Format</span>
                <div class="image-convert-format-grid" role="radiogroup" aria-label="Output format">
                  <label
                    v-for="format in FORMATS"
                    :key="format.value"
                    class="image-convert-format"
                    :class="{ 'image-convert-format--active': targetFormat === format.value }"
                  >
                    <input v-model="targetFormat" type="radio" name="image-convert-format" :value="format.value">
                    <span class="image-convert-format-icon" aria-hidden="true">
                      <AppIcon :name="format.icon" :size="15" :stroke-width="1.9" />
                    </span>
                    <span class="image-convert-format-copy">
                      <strong>{{ format.label }}</strong>
                      <small>.{{ format.extension }}</small>
                    </span>
                    <span class="image-convert-format-check" aria-hidden="true">
                      <AppIcon name="check" :size="12" :stroke-width="2.7" />
                    </span>
                  </label>
                </div>
              </div>

              <div v-if="supportsQuality" class="image-convert-section">
                <span class="image-convert-eyebrow">Quality</span>
                <label class="image-convert-slider">
                  <span>
                    <strong>{{ qualityFormatLabel }}</strong>
                    <small>{{ imageQuality }}</small>
                  </span>
                  <input v-model.number="imageQuality" type="range" min="1" max="100" step="1">
                </label>
                <span class="image-convert-help">Higher keeps more detail; lower makes smaller files.</span>
              </div>

              <div class="image-convert-section">
                <span class="image-convert-eyebrow">Save to</span>
                <div class="image-convert-segments" role="group" aria-label="Output destination">
                  <button
                    type="button"
                    :class="{ active: destinationMode === 'sameFolder' }"
                    :aria-pressed="destinationMode === 'sameFolder'"
                    @click="destinationMode = 'sameFolder'"
                  >
                    <AppIcon name="folder" :size="14" :stroke-width="1.9" />
                    <span>Same Folder</span>
                  </button>
                  <button
                    v-if="canUseOtherPane"
                    type="button"
                    :class="{ active: destinationMode === 'otherPane' }"
                    :aria-pressed="destinationMode === 'otherPane'"
                    @click="destinationMode = 'otherPane'"
                  >
                    <AppIcon name="open-other-pane" :size="14" :stroke-width="1.9" />
                    <span>Other Pane</span>
                  </button>
                </div>
                <span class="image-convert-destination">{{ destinationLabel }}</span>
              </div>

              <div class="image-convert-section">
                <span class="image-convert-eyebrow">If a name already exists</span>
                <div class="image-convert-segments" role="radiogroup" aria-label="Name conflicts">
                  <button
                    v-for="policy in CONFLICT_POLICIES"
                    :key="policy.value"
                    type="button"
                    :class="{ active: conflictPolicy === policy.value }"
                    :aria-pressed="conflictPolicy === policy.value"
                    @click="conflictPolicy = policy.value"
                  >
                    <span>{{ policy.label }}</span>
                  </button>
                </div>
              </div>
            </section>

            <section class="image-convert-preview" :style="previewRowStyle" aria-label="Conversion preview">
              <div class="image-convert-preview-head">
                <span>Source</span>
                <span>Output</span>
                <span>Status</span>
              </div>

              <div
                ref="previewListRef"
                class="image-convert-preview-list"
                @scroll.passive="handlePreviewScroll"
              >
                <div
                  v-if="virtualPreviewRange.paddingBefore > 0"
                  :style="{ height: `${virtualPreviewRange.paddingBefore}px` }"
                  aria-hidden="true"
                ></div>

                <article
                  v-for="row in virtualPreviewRows"
                  :key="row.id"
                  class="image-convert-row"
                  :class="`image-convert-row--${row.status}`"
                >
                  <span class="image-convert-row-source">{{ row.entry.name }}</span>
                  <span class="image-convert-row-output">{{ row.nextName }}</span>
                  <span class="image-convert-row-status">{{ row.message }}</span>
                </article>

                <div
                  v-if="virtualPreviewRange.paddingAfter > 0"
                  :style="{ height: `${virtualPreviewRange.paddingAfter}px` }"
                  aria-hidden="true"
                ></div>
              </div>
            </section>
          </div>

          <footer class="image-convert-footer">
            <button type="button" class="app-button app-button--subtle" @click="cancel">Cancel</button>
            <button
              type="button"
              class="app-button app-button--primary"
              :disabled="!canConvert"
              @click="convert"
            >
              {{ primaryLabel }}
            </button>
          </footer>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.image-convert-overlay {
  position: fixed;
  z-index: 5100;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 28px;
  background: var(--overlay-bg);
}

.image-convert-panel {
  display: grid;
  width: min(980px, calc(100vw - 56px));
  max-height: min(760px, calc(100vh - 56px));
  grid-template-rows: auto minmax(0, 1fr) auto;
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 11px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
}

.image-convert-header {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
  border-bottom: 1px solid transparent;
  padding: 16px 18px;
}

.image-convert-panel--content-scrollable .image-convert-header {
  border-bottom-color: var(--separator);
}

.image-convert-title-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 11px;
}

.image-convert-icon {
  display: inline-flex;
  width: 34px;
  height: 34px;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  border-radius: 9px;
  background: color-mix(in srgb, var(--accent) 14%, transparent);
  color: var(--accent);
}

.image-convert-title-copy {
  min-width: 0;
}

.image-convert-title-copy h2 {
  margin: 0;
  color: var(--text);
  font-size: 15px;
  font-weight: 650;
  letter-spacing: 0;
}

.image-convert-title-copy p {
  margin: 3px 0 0;
  color: var(--text-muted);
  font-size: 12px;
}

.image-convert-summary {
  display: inline-grid;
  min-width: 112px;
  justify-items: end;
  gap: 2px;
  color: var(--text-muted);
  font-size: 11px;
}

.image-convert-summary strong {
  color: var(--text);
  font-size: 13px;
}

.image-convert-summary--ready strong {
  color: var(--success);
}

.image-convert-layout {
  display: grid;
  min-height: 0;
  grid-template-columns: minmax(260px, 330px) minmax(0, 1fr);
}

.image-convert-controls {
  min-width: 0;
  overflow: auto;
  border-right: 1px solid var(--separator);
  padding: 16px;
}

.image-convert-section + .image-convert-section {
  margin-top: 18px;
}

.image-convert-hint {
  margin: 0 0 16px;
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.4;
}

.image-convert-help {
  display: block;
  margin-top: 7px;
  color: var(--text-faint);
  font-size: 11px;
  line-height: 1.35;
}

.image-convert-eyebrow {
  display: block;
  margin-bottom: 8px;
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: uppercase;
}

.image-convert-format-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.image-convert-format {
  position: relative;
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 8px;
  border: 1px solid var(--control-border);
  border-radius: 8px;
  padding: 9px;
  background: color-mix(in srgb, var(--control-glass) 72%, transparent);
  box-shadow: var(--input-shadow);
  cursor: pointer;
}

.image-convert-format input {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: -1px;
  overflow: hidden;
  clip-path: inset(50%);
}

.image-convert-format--active {
  border-color: color-mix(in srgb, var(--accent) 52%, var(--control-border));
  background: color-mix(in srgb, var(--accent) 12%, var(--control-glass));
}

.image-convert-format:focus-within {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.image-convert-format-icon {
  display: flex;
  flex: 0 0 auto;
  color: var(--accent);
}

.image-convert-format-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.image-convert-format-copy strong {
  font-size: 12px;
  font-weight: 650;
}

.image-convert-format-copy small {
  color: var(--text-muted);
  font-size: 11px;
}

.image-convert-format-check {
  margin-left: auto;
  color: var(--accent);
  opacity: 0;
}

.image-convert-format--active .image-convert-format-check {
  opacity: 1;
}

.image-convert-slider {
  display: grid;
  gap: 8px;
  border: 1px solid var(--control-border);
  border-radius: 9px;
  padding: 10px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
}

.image-convert-slider span {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  color: var(--text-muted);
  font-size: 12px;
}

.image-convert-slider strong {
  color: var(--text);
  font-size: 12px;
}

.image-convert-slider input {
  width: 100%;
}

.image-convert-segments {
  display: inline-flex;
  width: 100%;
  min-width: 0;
  gap: 4px;
  border-radius: 9px;
  padding: 3px;
  background: var(--control-bg);
  box-shadow: var(--control-inset);
}

.image-convert-segments button {
  display: inline-flex;
  min-width: 0;
  flex: 1 1 0;
  align-items: center;
  justify-content: center;
  gap: 6px;
  border-radius: 7px;
  padding: 7px 8px;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
}

.image-convert-segments button.active {
  background: color-mix(in srgb, var(--text) 9%, transparent);
  color: var(--text);
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.image-convert-destination {
  display: block;
  min-width: 0;
  margin-top: 7px;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.image-convert-preview {
  display: grid;
  min-width: 0;
  min-height: 0;
  grid-template-rows: 32px minmax(0, 1fr);
}

.image-convert-preview-head,
.image-convert-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) 82px;
  align-items: center;
  gap: 14px;
}

.image-convert-preview-head {
  border-bottom: 1px solid var(--separator);
  padding: 0 14px;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 650;
}

.image-convert-preview-list {
  min-height: 0;
  overflow: auto;
}

.image-convert-row {
  height: var(--image-convert-row-height);
  border-bottom: 1px solid color-mix(in srgb, var(--separator) 56%, transparent);
  padding: 0 14px;
  color: var(--text);
  font-size: 12px;
}

.image-convert-row:nth-child(even) {
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
}

.image-convert-row-source,
.image-convert-row-output {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.image-convert-row-output {
  color: var(--text);
  font-weight: 600;
}

.image-convert-row-status {
  justify-self: start;
  color: var(--text-muted);
  font-size: 11px;
}

.image-convert-row--replace .image-convert-row-status {
  color: var(--warning);
}

.image-convert-row--skip {
  color: var(--text-faint);
}

.image-convert-footer {
  display: flex;
  justify-content: flex-end;
  gap: 9px;
  border-top: 1px solid transparent;
  padding: 13px 16px;
}

.image-convert-panel--content-scrollable .image-convert-footer {
  border-top-color: var(--separator);
}

.image-convert-dialog-enter-active,
.image-convert-dialog-leave-active {
  transition: opacity 180ms ease;
}

.image-convert-dialog-enter-from,
.image-convert-dialog-leave-to {
  opacity: 0;
}

.image-convert-dialog-enter-active .image-convert-panel,
.image-convert-dialog-leave-active .image-convert-panel {
  transition: transform 160ms cubic-bezier(0.2, 0, 0, 1), opacity 140ms ease;
}

.image-convert-dialog-enter-from .image-convert-panel,
.image-convert-dialog-leave-to .image-convert-panel {
  opacity: 0;
  transform: translateY(8px) scale(0.985);
}

@media (max-width: 760px) {
  .image-convert-overlay {
    padding: 12px;
  }

  .image-convert-panel {
    width: calc(100vw - 24px);
    max-height: calc(100vh - 24px);
  }

  .image-convert-layout {
    grid-template-columns: minmax(0, 1fr);
  }

  .image-convert-controls {
    border-right: 0;
    border-bottom: 1px solid var(--separator);
  }

  .image-convert-preview {
    min-height: 260px;
  }

  .image-convert-preview-head,
  .image-convert-row {
    grid-template-columns: minmax(0, 1fr) 88px;
  }

  .image-convert-preview-head span:nth-child(2),
  .image-convert-row-output {
    display: none;
  }
}
</style>
