<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';

const DESKTOP_PREVIEW_ROW_HEIGHT = 34;
const MOBILE_PREVIEW_ROW_HEIGHT = 74;
const PREVIEW_OVERSCAN_ROWS = 12;

const MODES = [
  { value: 'replace', label: 'Replace', icon: 'search', description: 'Find and replace text' },
  { value: 'add', label: 'Add Text', icon: 'plus', description: 'Add a prefix or suffix' },
  { value: 'number', label: 'Number', icon: 'sort', description: 'Append a sequence number' },
  { value: 'case', label: 'Case', icon: 'file-text', description: 'Change letter casing' },
];

const CASE_MODES = [
  { value: 'lower', label: 'lowercase' },
  { value: 'upper', label: 'UPPERCASE' },
  { value: 'title', label: 'Title Case' },
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
});

const emit = defineEmits(['cancel', 'rename']);

const panelRef = ref(null);
const methodTriggerRef = ref(null);
const methodDropdownRef = ref(null);
const methodDropdownOpen = ref(false);
const methodDropdownRect = reactive({ top: 0, left: 0, width: 0 });
const replaceInput = ref(null);
const prefixInput = ref(null);
const templateInput = ref(null);
const previewListRef = ref(null);
const previewScrollTop = ref(0);
const previewViewportHeight = ref(0);
const previewRowHeight = ref(DESKTOP_PREVIEW_ROW_HEIGHT);

const mode = ref('replace');
const findText = ref('');
const replaceText = ref('');
const prefixText = ref('');
const suffixText = ref('');
const numberTemplate = ref('{name} {n}');
const startNumber = ref(1);
const numberPadding = ref(2);
const keepExtensions = ref(true);
const matchCase = ref(false);
const useRegex = ref(false);
const caseMode = ref('lower');

const hasFileEntries = computed(() => props.entries.some((entry) => entry?.kind === 'file'));
const currentMode = computed(() => MODES.find((candidate) => candidate.value === mode.value) || MODES[0]);
const selectedLabel = computed(() => {
  const count = props.entries.length;

  return `${count} ${count === 1 ? 'item' : 'items'} selected`;
});

const regexError = computed(() => {
  if (mode.value !== 'replace' || !useRegex.value || !findText.value) {
    return '';
  }

  try {
    replacementExpression();
    return '';
  } catch (error) {
    return error?.message || 'Invalid regular expression.';
  }
});

const baseRows = computed(() => props.entries.map((entry, index) => {
  const directory = entry.directory || parentDirectoryForPath(entry.path);
  const nextName = nextNameForEntry(entry, index);

  return {
    id: entry.path || `${entry.name}-${index}`,
    entry,
    index,
    directory,
    nextName,
    changed: nextName !== entry.name,
    preliminaryError: nameError(nextName),
  };
}));

const previewRows = computed(() => {
  const finalCounts = new Map();

  for (const row of baseRows.value) {
    const key = rowKey(row.directory, row.nextName);
    finalCounts.set(key, (finalCounts.get(key) || 0) + 1);
  }

  return baseRows.value.map((row) => {
    const errors = [];

    if (regexError.value) {
      errors.push(regexError.value);
    }

    if (row.preliminaryError) {
      errors.push(row.preliminaryError);
    }

    if (finalCounts.get(rowKey(row.directory, row.nextName)) > 1) {
      errors.push('Duplicate result');
    }

    if (row.changed && existingNameSet(row.directory).has(normalizeName(row.nextName))) {
      errors.push('Name already exists');
    }

    const status = errors.length > 0
      ? 'error'
      : row.changed
        ? 'ready'
        : 'unchanged';

    return {
      ...row,
      status,
      message: errors[0] || (row.changed ? 'Ready' : 'Unchanged'),
    };
  });
});

const previewSummary = computed(() => {
  let ready = 0;
  let error = 0;
  let unchanged = 0;

  for (const row of previewRows.value) {
    if (row.status === 'ready') {
      ready += 1;
    } else if (row.status === 'error') {
      error += 1;
    } else {
      unchanged += 1;
    }
  }

  return { ready, error, unchanged };
});
const changedRows = computed(() => previewRows.value.filter((row) => row.status === 'ready'));
const canRename = computed(() => previewSummary.value.ready > 0 && previewSummary.value.error === 0);
const primaryLabel = computed(() => (
  previewSummary.value.ready === 1 ? 'Rename 1 Item' : `Rename ${previewSummary.value.ready} Items`
));
const summaryTone = computed(() => {
  if (previewSummary.value.error > 0) {
    return 'error';
  }

  return previewSummary.value.ready > 0 ? 'ready' : 'idle';
});
const summaryText = computed(() => {
  if (previewSummary.value.error > 0) {
    return `${previewSummary.value.error} ${previewSummary.value.error === 1 ? 'issue' : 'issues'}`;
  }

  if (previewSummary.value.ready > 0) {
    return `${previewSummary.value.ready} ${previewSummary.value.ready === 1 ? 'rename' : 'renames'}`;
  }

  return 'No changes';
});
const virtualPreviewRange = computed(() => {
  const rowHeight = Math.max(1, previewRowHeight.value);
  const count = previewRows.value.length;
  const viewportHeight = Math.max(previewViewportHeight.value || rowHeight * 12, rowHeight);
  const maxStart = Math.max(0, count - 1);
  const start = Math.min(
    maxStart,
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
  '--batch-rename-row-height': `${previewRowHeight.value}px`,
}));
let previewResizeObserver = null;

function resetState() {
  mode.value = 'replace';
  findText.value = '';
  replaceText.value = '';
  prefixText.value = '';
  suffixText.value = '';
  numberTemplate.value = '{name} {n}';
  startNumber.value = 1;
  numberPadding.value = props.entries.length >= 100 ? 3 : 2;
  keepExtensions.value = true;
  matchCase.value = false;
  useRegex.value = false;
  caseMode.value = 'lower';
}

function focusActiveField() {
  nextTick(() => {
    if (mode.value === 'replace') {
      replaceInput.value?.focus?.({ preventScroll: true });
      return;
    }

    if (mode.value === 'add') {
      prefixInput.value?.focus?.({ preventScroll: true });
      return;
    }

    if (mode.value === 'number') {
      templateInput.value?.focus?.({ preventScroll: true });
      return;
    }

    panelRef.value?.focus?.({ preventScroll: true });
  });
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

    if (typeof ResizeObserver === 'undefined' || !previewListRef.value) {
      return;
    }

    previewResizeObserver = new ResizeObserver(updatePreviewMetrics);
    previewResizeObserver.observe(previewListRef.value);
  });
}

function handlePreviewScroll(event) {
  previewScrollTop.value = event.currentTarget?.scrollTop || 0;
}

function chooseMode(nextMode) {
  mode.value = nextMode;
  focusActiveField();
}

function openMethodDropdown() {
  const el = methodTriggerRef.value;

  if (el) {
    const rect = el.getBoundingClientRect();
    methodDropdownRect.top = rect.bottom + 5;
    methodDropdownRect.left = rect.left;
    methodDropdownRect.width = rect.width;
  }

  methodDropdownOpen.value = true;
  nextTick(() => {
    document.addEventListener('pointerdown', handleMethodOutsideClick, { capture: true });
  });
}

function closeMethodDropdown() {
  methodDropdownOpen.value = false;
  document.removeEventListener('pointerdown', handleMethodOutsideClick, { capture: true });
}

function toggleMethodDropdown() {
  if (methodDropdownOpen.value) {
    closeMethodDropdown();
  } else {
    openMethodDropdown();
  }
}

function selectMethod(value) {
  closeMethodDropdown();
  chooseMode(value);
}

function handleMethodOutsideClick(event) {
  if (
    !methodTriggerRef.value?.contains(event.target) &&
    !methodDropdownRef.value?.contains(event.target)
  ) {
    closeMethodDropdown();
  }
}

function normalizeName(value) {
  return String(value || '').toLocaleLowerCase();
}

function rowKey(directory, name) {
  return `${String(directory || '')}\u0000${normalizeName(name)}`;
}

function existingNameSet(directory) {
  const names = props.existingNamesByDirectory[directory] || [];

  return new Set(names.map(normalizeName));
}

function escapeRegExp(value) {
  return String(value).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function replacementExpression() {
  const source = useRegex.value ? findText.value : escapeRegExp(findText.value);
  const flags = matchCase.value ? 'g' : 'gi';

  return new RegExp(source, flags);
}

function splitName(entry) {
  const name = String(entry?.name || '');

  if (entry?.kind !== 'file') {
    return { stem: name, extension: '' };
  }

  const dotIndex = name.lastIndexOf('.');

  if (dotIndex <= 0 || dotIndex === name.length - 1) {
    return { stem: name, extension: '' };
  }

  return {
    stem: name.slice(0, dotIndex),
    extension: name.slice(dotIndex),
  };
}

function sequenceText(index) {
  const start = Math.max(0, Math.trunc(Number(startNumber.value) || 0));
  const padding = Math.max(1, Math.min(8, Math.trunc(Number(numberPadding.value) || 1)));

  return String(start + index).padStart(padding, '0');
}

function titleCase(value) {
  return String(value)
    .toLocaleLowerCase()
    .replace(/(^|[\s._-])(\S)/g, (match, lead, letter) => `${lead}${letter.toLocaleUpperCase()}`);
}

function transformValue(value, entry, index) {
  if (mode.value === 'replace') {
    if (!findText.value) {
      return value;
    }

    return String(value).replace(replacementExpression(), replaceText.value);
  }

  if (mode.value === 'add') {
    return `${prefixText.value}${value}${suffixText.value}`;
  }

  if (mode.value === 'number') {
    return String(numberTemplate.value || '{name} {n}')
      .replaceAll('{name}', value)
      .replaceAll('{n}', sequenceText(index));
  }

  if (mode.value === 'case') {
    if (caseMode.value === 'upper') {
      return String(value).toLocaleUpperCase();
    }

    if (caseMode.value === 'title') {
      return titleCase(value);
    }

    return String(value).toLocaleLowerCase();
  }

  return value;
}

function nextNameForEntry(entry, index) {
  try {
    const parts = splitName(entry);
    const preserveExtension = keepExtensions.value && entry?.kind === 'file';
    const source = preserveExtension ? parts.stem : String(entry?.name || '');
    const transformed = transformValue(source, entry, index).trim();

    return preserveExtension ? `${transformed}${parts.extension}` : transformed;
  } catch {
    return String(entry?.name || '');
  }
}

function nameError(name) {
  const value = String(name || '');

  if (!value.trim()) {
    return 'Name is empty';
  }

  if (/[\\/]/.test(value)) {
    return 'Folder separators are not allowed';
  }

  if (value === '.' || value === '..') {
    return 'Reserved name';
  }

  if (value.length > 255) {
    return 'Name is too long';
  }

  return '';
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
      return value;
    }

    const parentIndex = objectPath.lastIndexOf('/');
    return parentIndex < 0
      ? `remote://${volumeId}/`
      : `remote://${volumeId}/${objectPath.slice(0, parentIndex)}`;
  }

  const slashIndex = value.lastIndexOf('/');
  return slashIndex <= 0 ? '/' : value.slice(0, slashIndex);
}

function cancel() {
  closeMethodDropdown();
  emit('cancel');
}

function rename() {
  if (!canRename.value) {
    return;
  }

  emit('rename', {
    renames: changedRows.value.map((row) => ({
      entry: row.entry,
      path: row.entry.path,
      name: row.entry.name,
      nextName: row.nextName,
      directory: row.directory,
    })),
  });
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault();

    if (methodDropdownOpen.value) {
      closeMethodDropdown();
      return;
    }

    cancel();
    return;
  }

  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    rename();
  }
}

watch(() => props.visible, (visible) => {
  if (!visible) {
    closeMethodDropdown();
    return;
  }

  resetState();
  previewScrollTop.value = 0;
  focusActiveField();
  observePreviewList();
});

watch(mode, focusActiveField);

onMounted(() => {
  observePreviewList();
  window.addEventListener('resize', updatePreviewMetrics);

  if (props.visible) {
    focusActiveField();
  }
});

onBeforeUnmount(() => {
  closeMethodDropdown();
  previewResizeObserver?.disconnect?.();
  window.removeEventListener('resize', updatePreviewMetrics);
});
</script>

<template>
  <Teleport to="body">
    <Transition name="batch-rename-dialog">
      <div
        v-if="visible"
        class="batch-rename-overlay"
        role="presentation"
        @pointerdown.self="cancel"
        @keydown.stop="handleKeydown"
      >
        <section
          ref="panelRef"
          class="batch-rename-panel"
          role="dialog"
          aria-modal="true"
          aria-labelledby="batch-rename-title"
          tabindex="-1"
        >
          <header class="batch-rename-header">
            <div class="batch-rename-title-row">
              <span class="batch-rename-icon" aria-hidden="true">
                <AppIcon name="file-text" :size="20" :stroke-width="1.9" />
              </span>
              <div class="batch-rename-title-copy">
                <h2 id="batch-rename-title">Batch Rename</h2>
                <p>{{ selectedLabel }}</p>
              </div>
            </div>

            <div class="batch-rename-summary" :class="`batch-rename-summary--${summaryTone}`">
              <strong>{{ summaryText }}</strong>
              <span>{{ previewSummary.unchanged }} unchanged</span>
            </div>
          </header>

          <div class="batch-rename-layout">
            <section class="batch-rename-controls" aria-label="Rename rules">
              <div class="batch-rename-section">
                <span class="batch-rename-eyebrow">Method</span>
                <div ref="methodTriggerRef" class="batch-rename-select">
                  <button
                    type="button"
                    class="batch-rename-select-trigger"
                    :class="{ 'batch-rename-select-trigger--open': methodDropdownOpen }"
                    aria-haspopup="listbox"
                    :aria-expanded="methodDropdownOpen"
                    @click="toggleMethodDropdown"
                  >
                    <span class="batch-rename-select-icon" aria-hidden="true">
                      <AppIcon :name="currentMode.icon" :size="17" :stroke-width="1.8" />
                    </span>
                    <span class="batch-rename-select-body">
                      <span class="batch-rename-select-label">{{ currentMode.label }}</span>
                      <span class="batch-rename-select-desc">{{ currentMode.description }}</span>
                    </span>
                    <span class="batch-rename-select-chevron" aria-hidden="true">
                      <AppIcon name="chevron-down" :size="14" :stroke-width="2.2" />
                    </span>
                  </button>
                </div>

                <Teleport to="body">
                  <div
                    v-if="methodDropdownOpen"
                    ref="methodDropdownRef"
                    class="batch-rename-select-dropdown"
                    role="listbox"
                    :style="{
                      top: `${methodDropdownRect.top}px`,
                      left: `${methodDropdownRect.left}px`,
                      width: `${methodDropdownRect.width}px`,
                    }"
                  >
                    <button
                      v-for="candidate in MODES"
                      :key="candidate.value"
                      type="button"
                      class="batch-rename-select-option"
                      :class="{ 'batch-rename-select-option--active': mode === candidate.value }"
                      role="option"
                      :aria-selected="mode === candidate.value"
                      @click="selectMethod(candidate.value)"
                    >
                      <span class="batch-rename-select-option-icon" aria-hidden="true">
                        <AppIcon :name="candidate.icon" :size="16" :stroke-width="1.8" />
                      </span>
                      <span class="batch-rename-select-option-body">
                        <span class="batch-rename-select-option-label">{{ candidate.label }}</span>
                        <span class="batch-rename-select-option-desc">{{ candidate.description }}</span>
                      </span>
                      <span
                        v-if="mode === candidate.value"
                        class="batch-rename-select-option-check"
                        aria-hidden="true"
                      >
                        <AppIcon name="check" :size="13" :stroke-width="2.6" />
                      </span>
                    </button>
                  </div>
                </Teleport>
              </div>

              <div class="batch-rename-section">
                <span class="batch-rename-eyebrow">Options</span>

                <div v-if="mode === 'replace'" class="batch-rename-fields">
                  <label class="batch-rename-field">
                    <span>Find</span>
                    <input ref="replaceInput" v-model="findText" type="text" autocomplete="off">
                  </label>
                  <label class="batch-rename-field">
                    <span>Replace With</span>
                    <input v-model="replaceText" type="text" autocomplete="off">
                  </label>
                  <div class="batch-rename-toggle-row">
                    <label class="batch-rename-option">
                      <span class="batch-rename-option-label">Match case</span>
                      <input v-model="matchCase" class="batch-rename-switch-input" type="checkbox">
                      <span class="batch-rename-switch-ui" aria-hidden="true"></span>
                    </label>
                    <label class="batch-rename-option">
                      <span class="batch-rename-option-label">Regular expression</span>
                      <input v-model="useRegex" class="batch-rename-switch-input" type="checkbox">
                      <span class="batch-rename-switch-ui" aria-hidden="true"></span>
                    </label>
                  </div>
                </div>

                <div v-else-if="mode === 'add'" class="batch-rename-fields">
                  <label class="batch-rename-field">
                    <span>Prefix</span>
                    <input ref="prefixInput" v-model="prefixText" type="text" autocomplete="off">
                  </label>
                  <label class="batch-rename-field">
                    <span>Suffix</span>
                    <input v-model="suffixText" type="text" autocomplete="off">
                  </label>
                </div>

                <div v-else-if="mode === 'number'" class="batch-rename-fields">
                  <label class="batch-rename-field">
                    <span>Template</span>
                    <input ref="templateInput" v-model="numberTemplate" type="text" autocomplete="off">
                  </label>
                  <div class="batch-rename-number-grid">
                    <label class="batch-rename-field">
                      <span>Start</span>
                      <input v-model.number="startNumber" type="number" min="0" step="1">
                    </label>
                    <label class="batch-rename-field">
                      <span>Padding</span>
                      <input v-model.number="numberPadding" type="number" min="1" max="8" step="1">
                    </label>
                  </div>
                  <div class="batch-rename-token-field">
                    <span class="batch-rename-token-label">Insert token</span>
                    <div class="batch-rename-token-row">
                      <button type="button" @click="numberTemplate = `${numberTemplate}{name}`">
                        <AppIcon name="plus" :size="11" :stroke-width="2.4" />
                        <span>{name}</span>
                      </button>
                      <button type="button" @click="numberTemplate = `${numberTemplate}{n}`">
                        <AppIcon name="plus" :size="11" :stroke-width="2.4" />
                        <span>{n}</span>
                      </button>
                    </div>
                  </div>
                </div>

                <div v-else class="batch-rename-fields">
                  <div class="batch-rename-case-grid" role="radiogroup" aria-label="Case conversion">
                    <label
                      v-for="candidate in CASE_MODES"
                      :key="candidate.value"
                      class="batch-rename-case"
                      :class="{ 'batch-rename-case--active': caseMode === candidate.value }"
                    >
                      <input v-model="caseMode" type="radio" name="batch-rename-case" :value="candidate.value">
                      <span class="batch-rename-case-label">{{ candidate.label }}</span>
                      <span class="batch-rename-case-check" aria-hidden="true">
                        <AppIcon name="check" :size="13" :stroke-width="2.6" />
                      </span>
                    </label>
                  </div>
                </div>
              </div>

              <label
                v-if="hasFileEntries"
                class="batch-rename-switch"
              >
                <span>
                  <strong>Keep file extensions</strong>
                  <small>Changes apply before the final dot.</small>
                </span>
                <input v-model="keepExtensions" class="batch-rename-switch-input" type="checkbox">
                <span class="batch-rename-switch-ui" aria-hidden="true"></span>
              </label>
            </section>

            <section class="batch-rename-preview" :style="previewRowStyle" aria-label="Rename preview">
              <div class="batch-rename-preview-head">
                <span>Current Name</span>
                <span>New Name</span>
                <span>Status</span>
              </div>

              <div
                ref="previewListRef"
                class="batch-rename-preview-list"
                @scroll.passive="handlePreviewScroll"
              >
                <div
                  v-if="virtualPreviewRange.paddingBefore > 0"
                  class="batch-rename-preview-spacer"
                  :style="{ height: `${virtualPreviewRange.paddingBefore}px` }"
                  aria-hidden="true"
                ></div>
                <div
                  v-for="row in virtualPreviewRows"
                  :key="row.id"
                  class="batch-rename-preview-row"
                  :class="`batch-rename-preview-row--${row.status}`"
                >
                  <span class="batch-rename-old" :title="row.entry.name">
                    <AppIcon :name="row.entry.kind === 'directory' ? 'folder' : 'file'" :size="15" :stroke-width="1.8" />
                    <span>{{ row.entry.name }}</span>
                  </span>
                  <span class="batch-rename-new" :title="row.nextName">{{ row.nextName }}</span>
                  <span class="batch-rename-status">
                    <AppIcon
                      :name="row.status === 'error' ? 'alert' : row.status === 'ready' ? 'check' : 'minus'"
                      :size="14"
                      :stroke-width="2.1"
                    />
                    <span>{{ row.message }}</span>
                  </span>
                </div>
                <div
                  v-if="virtualPreviewRange.paddingAfter > 0"
                  class="batch-rename-preview-spacer"
                  :style="{ height: `${virtualPreviewRange.paddingAfter}px` }"
                  aria-hidden="true"
                ></div>
              </div>
            </section>
          </div>

          <footer class="batch-rename-actions">
            <button type="button" class="app-button" @click="cancel">Cancel</button>
            <button
              type="button"
              class="app-button app-button--primary"
              :disabled="!canRename"
              @click="rename"
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
.batch-rename-overlay {
  position: fixed;
  z-index: 5100;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 28px;
  background: var(--overlay-bg);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
}

.batch-rename-panel {
  display: grid;
  width: min(980px, calc(100vw - 48px));
  max-height: calc(100vh - 56px);
  grid-template-rows: auto minmax(0, 1fr) auto;
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 11px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
}

.batch-rename-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
  padding: 16px 16px 12px;
  border-bottom: 1px solid var(--hairline);
}

.batch-rename-title-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 10px;
}

.batch-rename-icon {
  display: grid;
  width: 28px;
  height: 28px;
  flex: 0 0 auto;
  place-items: center;
  color: var(--accent);
}

.batch-rename-title-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.batch-rename-title-copy h2 {
  margin: 0;
  font-size: 15px;
  font-weight: 720;
  letter-spacing: 0;
}

.batch-rename-title-copy p {
  overflow: hidden;
  margin: 0;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 560;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.batch-rename-summary {
  display: grid;
  min-width: 124px;
  justify-items: end;
  gap: 2px;
  color: var(--text-faint);
}

.batch-rename-summary strong {
  color: var(--text);
  font-size: 12.5px;
  font-weight: 720;
}

.batch-rename-summary span {
  font-size: 11px;
  font-weight: 620;
}

.batch-rename-summary--ready strong {
  color: var(--accent);
}

.batch-rename-summary--error strong {
  color: var(--danger);
}

.batch-rename-layout {
  display: grid;
  min-height: 0;
  grid-template-columns: minmax(260px, 320px) minmax(0, 1fr);
}

.batch-rename-controls {
  display: grid;
  align-content: start;
  gap: 18px;
  min-width: 0;
  overflow-y: auto;
  padding: 16px 14px;
  border-right: 1px solid var(--hairline);
}

.batch-rename-section {
  display: grid;
  gap: 10px;
  min-width: 0;
}

.batch-rename-eyebrow {
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 760;
  letter-spacing: 0.05em;
  text-transform: uppercase;
}

.batch-rename-select {
  position: relative;
}

.batch-rename-select-trigger {
  display: flex;
  width: 100%;
  height: 48px;
  align-items: center;
  gap: 10px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 0 12px 0 13px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text);
  text-align: left;
  transition: border-color 120ms ease, box-shadow 120ms ease;
}

.batch-rename-select-trigger:hover {
  border-color: var(--control-border);
}

.batch-rename-select-trigger--open {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.batch-rename-select-icon {
  display: flex;
  flex-shrink: 0;
  color: var(--accent);
}

.batch-rename-select-body {
  display: flex;
  flex: 1;
  min-width: 0;
  flex-direction: column;
  gap: 2px;
}

.batch-rename-select-label {
  overflow: hidden;
  font-size: 13px;
  font-weight: 650;
  line-height: 1;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.batch-rename-select-desc {
  overflow: hidden;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
  line-height: 1;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.batch-rename-select-chevron {
  display: flex;
  flex-shrink: 0;
  color: var(--icon);
  transition: transform 160ms cubic-bezier(0.2, 0, 0, 1);
}

.batch-rename-select-trigger--open .batch-rename-select-chevron {
  transform: rotate(180deg);
}

.batch-rename-select-dropdown {
  position: fixed;
  z-index: 9000;
  overflow-y: auto;
  max-height: 320px;
  padding: 4px;
  border: 1px solid var(--control-border);
  border-radius: 11px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  animation: batch-rename-select-dropdown-in 130ms cubic-bezier(0.2, 0, 0, 1) forwards;
}

@keyframes batch-rename-select-dropdown-in {
  from {
    opacity: 0;
    transform: translateY(-5px) scale(0.98);
    transform-origin: top center;
  }

  to {
    opacity: 1;
    transform: none;
  }
}

.batch-rename-select-option {
  display: flex;
  width: 100%;
  align-items: center;
  gap: 10px;
  border-radius: 7px;
  padding: 9px 8px;
  background: transparent;
  color: var(--text);
  text-align: left;
  transition: background 80ms ease;
}

.batch-rename-select-option:hover {
  background: var(--btn-hover);
}

.batch-rename-select-option--active {
  background: rgb(var(--accent-rgb) / 0.08);
}

.batch-rename-select-option-icon {
  display: flex;
  width: 30px;
  height: 30px;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  border-radius: 8px;
  background: rgb(var(--accent-rgb) / 0.1);
  color: var(--accent);
}

.batch-rename-select-option--active .batch-rename-select-option-icon {
  background: rgb(var(--accent-rgb) / 0.16);
}

.batch-rename-select-option-body {
  display: flex;
  flex: 1;
  min-width: 0;
  flex-direction: column;
  gap: 2px;
}

.batch-rename-select-option-label {
  font-size: 13px;
  font-weight: 580;
  line-height: 1;
}

.batch-rename-select-option-desc {
  overflow: hidden;
  color: var(--text-faint);
  font-size: 11px;
  line-height: 1;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.batch-rename-select-option-check {
  display: flex;
  flex-shrink: 0;
  color: var(--accent);
}

.batch-rename-fields {
  display: grid;
  gap: 10px;
  min-width: 0;
}

.batch-rename-field {
  display: grid;
  gap: 7px;
  min-width: 0;
}

.batch-rename-field > span {
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 700;
}

.batch-rename-field input {
  width: 100%;
  height: 34px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 0 10px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text);
  font-size: 13px;
  font-weight: 520;
  outline: 0;
}

.batch-rename-field input:focus {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.batch-rename-number-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 9px;
}

.batch-rename-token-field {
  display: grid;
  gap: 7px;
}

.batch-rename-token-label {
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 700;
}

.batch-rename-token-row {
  display: flex;
  flex-wrap: wrap;
  gap: 7px;
}

.batch-rename-token-row button {
  display: inline-flex;
  height: 28px;
  align-items: center;
  gap: 5px;
  border: 1px solid var(--input-border);
  border-radius: 999px;
  padding: 0 11px 0 9px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 620;
  font-variant-ligatures: none;
  font-family: ui-monospace, "SF Mono", "JetBrains Mono", Menlo, monospace;
  transition: border-color 120ms ease, background 120ms ease, color 120ms ease;
}

.batch-rename-token-row button :deep(svg) {
  color: var(--accent);
}

.batch-rename-token-row button:hover {
  border-color: var(--accent-border);
  background: rgb(var(--accent-rgb) / 0.1);
  color: var(--text);
}

.batch-rename-toggle-row {
  display: grid;
  gap: 7px;
}

.batch-rename-option {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 10px;
  min-height: 30px;
  padding: 0 2px;
  color: var(--text-muted);
  font-size: 12.5px;
  font-weight: 600;
  transition: color 120ms ease;
}

.batch-rename-option:hover {
  color: var(--text);
}

.batch-rename-option-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.batch-rename-case-grid {
  display: grid;
  gap: 7px;
}

.batch-rename-case {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 9px;
  min-height: 36px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 0 11px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text-muted);
  font-size: 12.5px;
  font-weight: 600;
  transition: border-color 120ms ease, background 120ms ease, color 120ms ease;
}

.batch-rename-case input {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  clip-path: inset(50%);
  white-space: nowrap;
}

.batch-rename-case-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.batch-rename-case-check {
  display: flex;
  flex: 0 0 auto;
  color: var(--accent);
  opacity: 0;
  transform: scale(0.8);
  transition: opacity 120ms ease, transform 120ms ease;
}

.batch-rename-case:hover {
  border-color: var(--control-border);
  color: var(--text);
}

.batch-rename-case:focus-within {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.batch-rename-case--active {
  border-color: var(--accent-border);
  background: rgb(var(--accent-rgb) / 0.09);
  color: var(--text);
}

.batch-rename-case--active .batch-rename-case-check {
  opacity: 1;
  transform: none;
}

.batch-rename-switch {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  min-height: 50px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 9px 10px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
}

.batch-rename-switch > span:first-child {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.batch-rename-switch strong,
.batch-rename-switch small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.batch-rename-switch strong {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 700;
}

.batch-rename-switch small {
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
}

.batch-rename-switch-input {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  clip-path: inset(50%);
  white-space: nowrap;
}

.batch-rename-switch-ui {
  position: relative;
  display: block;
  width: 42px;
  height: 24px;
  flex: 0 0 42px;
  border: 1px solid var(--input-border);
  border-radius: 999px;
  background: color-mix(in srgb, var(--text) 9%, transparent);
  box-shadow: var(--input-shadow);
}

.batch-rename-switch-ui::after {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: color-mix(in srgb, var(--text) 78%, transparent);
  box-shadow: 0 1px 4px rgb(0 0 0 / 0.28);
  content: "";
  transition: transform 120ms ease, background 120ms ease;
}

.batch-rename-switch-input:checked + .batch-rename-switch-ui {
  border-color: var(--accent-border);
  background: var(--accent);
}

.batch-rename-switch-input:checked + .batch-rename-switch-ui::after {
  background: #fff;
  transform: translateX(18px);
}

.batch-rename-switch-input:focus-visible + .batch-rename-switch-ui {
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.batch-rename-preview {
  display: grid;
  min-width: 0;
  min-height: 0;
  grid-template-rows: auto minmax(0, 1fr);
}

.batch-rename-preview-head,
.batch-rename-preview-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) minmax(118px, 0.45fr);
  gap: 12px;
  align-items: center;
}

.batch-rename-preview-head {
  min-height: 34px;
  padding: 0 14px;
  border-bottom: 1px solid var(--hairline);
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 760;
  text-transform: uppercase;
}

.batch-rename-preview-list {
  min-height: 0;
  overflow: auto;
}

.batch-rename-preview-row {
  height: var(--batch-rename-row-height);
  min-height: var(--batch-rename-row-height);
  padding: 0 14px;
  border-bottom: 1px solid var(--hairline);
  color: var(--text-muted);
  font-size: 12.5px;
  font-weight: 560;
}

.batch-rename-preview-spacer {
  pointer-events: none;
}

.batch-rename-preview-row--ready {
  background: rgb(var(--accent-rgb) / 0.045);
}

.batch-rename-preview-row--error {
  background: color-mix(in srgb, var(--danger) 9%, transparent);
}

.batch-rename-old,
.batch-rename-status {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 7px;
}

.batch-rename-old span,
.batch-rename-new,
.batch-rename-status span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.batch-rename-old :deep(svg) {
  flex: 0 0 auto;
  color: var(--icon);
}

.batch-rename-new {
  color: var(--text);
  font-weight: 650;
}

.batch-rename-preview-row--unchanged .batch-rename-new {
  color: var(--text-faint);
  font-weight: 560;
}

.batch-rename-status {
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 650;
}

.batch-rename-preview-row--ready .batch-rename-status {
  color: var(--accent);
}

.batch-rename-preview-row--error .batch-rename-status {
  color: var(--danger);
}

.batch-rename-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  padding: 12px 16px 16px;
  border-top: 1px solid var(--hairline);
}

.batch-rename-dialog-enter-active,
.batch-rename-dialog-leave-active {
  transition: opacity 120ms ease;
}

.batch-rename-dialog-enter-active .batch-rename-panel,
.batch-rename-dialog-leave-active .batch-rename-panel {
  transition: transform 120ms ease, opacity 120ms ease;
}

.batch-rename-dialog-enter-from,
.batch-rename-dialog-leave-to {
  opacity: 0;
}

.batch-rename-dialog-enter-from .batch-rename-panel,
.batch-rename-dialog-leave-to .batch-rename-panel {
  opacity: 0;
  transform: translateY(6px) scale(0.985);
}

@media (max-width: 820px) {
  .batch-rename-panel {
    width: min(620px, calc(100vw - 32px));
  }

  .batch-rename-layout {
    grid-template-columns: 1fr;
  }

  .batch-rename-controls {
    border-right: 0;
    border-bottom: 1px solid var(--hairline);
  }

  .batch-rename-preview {
    min-height: 280px;
  }
}

@media (max-width: 620px) {
  .batch-rename-overlay {
    padding: 14px;
  }

  .batch-rename-header {
    align-items: flex-start;
    flex-direction: column;
  }

  .batch-rename-summary {
    justify-items: start;
  }

  .batch-rename-preview-head,
  .batch-rename-preview-row {
    grid-template-columns: minmax(0, 1fr);
    gap: 4px;
    align-items: start;
    padding-top: 8px;
    padding-bottom: 8px;
  }

  .batch-rename-preview-head {
    display: none;
  }
}

@media (prefers-reduced-motion: reduce) {
  .batch-rename-dialog-enter-active,
  .batch-rename-dialog-leave-active,
  .batch-rename-dialog-enter-active .batch-rename-panel,
  .batch-rename-dialog-leave-active .batch-rename-panel {
    transition: none;
  }
}
</style>
