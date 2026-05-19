<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';

const ARCHIVE_FORMATS = [
  {
    value: 'zip',
    label: 'ZIP',
    detail: '.zip',
    description: 'Compressed .zip archive with optional password',
    extension: '.zip',
    icon: 'archive',
  },
  {
    value: 'tar',
    label: 'TAR',
    detail: '.tar',
    description: 'Uncompressed .tar archive',
    extension: '.tar',
    icon: 'archive',
  },
  {
    value: 'tarGz',
    label: 'TAR.GZ',
    detail: '.tar.gz',
    description: 'Gzip-compressed .tar.gz archive',
    extension: '.tar.gz',
    icon: 'archive',
  },
  {
    value: 'tarZst',
    label: 'TAR.ZST',
    detail: '.tar.zst',
    description: 'Zstandard-compressed .tar.zst archive',
    extension: '.tar.zst',
    icon: 'archive',
  },
];

const COMPRESSION_LEVELS = [
  {
    value: 'fast',
    label: 'Fast',
    detail: 'Lower ratio',
    description: 'Faster creation, larger archive',
    icon: 'sliders',
  },
  {
    value: 'balanced',
    label: 'Balanced',
    detail: 'Recommended',
    description: 'Recommended default for most archives',
    icon: 'sliders',
  },
  {
    value: 'best',
    label: 'Best',
    detail: 'Smaller file',
    description: 'Smaller archive, slower creation',
    icon: 'sliders',
  },
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
  directory: {
    type: String,
    default: '',
  },
  existingNames: {
    type: Array,
    default: () => [],
  },
});

const emit = defineEmits(['cancel', 'create']);

const nameInput = ref(null);
const archiveName = ref('');
const format = ref('zip');
const compressionLevel = ref('balanced');
const includeTopLevelDirectory = ref(true);
const replaceExisting = ref(false);
const usePassword = ref(false);
const password = ref('');
const confirmPassword = ref('');
const selectDropdown = ref({
  key: null,
  top: 0,
  left: 0,
  width: 0,
});
const formatTriggerRef = ref(null);
const compressionTriggerRef = ref(null);
const selectDropdownRef = ref(null);

const existingNameSet = computed(() => new Set(
  props.existingNames.map((name) => String(name || '').toLocaleLowerCase()),
));
const selectedDirectory = computed(() => props.entries.length === 1 && props.entries[0]?.kind === 'directory');
const canToggleTopLevel = computed(() => selectedDirectory.value);
const passwordAvailable = computed(() => format.value === 'zip');
const compressionDisabled = computed(() => format.value === 'tar');
const normalizedArchiveName = computed(() => normalizeArchiveName(archiveName.value, format.value));
const nameExists = computed(() => {
  const name = normalizedArchiveName.value;

  return Boolean(name && existingNameSet.value.has(name.toLocaleLowerCase()));
});
const passwordMismatch = computed(() => (
  usePassword.value &&
  passwordAvailable.value &&
  password.value.length > 0 &&
  confirmPassword.value.length > 0 &&
  password.value !== confirmPassword.value
));
const createDisabled = computed(() => (
  !normalizedArchiveName.value ||
  (nameExists.value && !replaceExisting.value) ||
  (usePassword.value && passwordAvailable.value && (!password.value || password.value !== confirmPassword.value))
));
const selectedLabel = computed(() => {
  const count = props.entries.length;

  if (count === 1) {
    return props.entries[0]?.kind === 'directory' ? '1 folder selected' : '1 file selected';
  }

  return `${count} items selected`;
});
const currentFormat = computed(() => (
  ARCHIVE_FORMATS.find((candidate) => candidate.value === format.value) || ARCHIVE_FORMATS[0]
));
const currentCompression = computed(() => {
  if (compressionDisabled.value) {
    return {
      value: 'none',
      label: 'None',
      detail: 'TAR only',
      description: 'Plain .tar archives are not compressed',
      icon: 'sliders',
    };
  }

  return COMPRESSION_LEVELS.find((candidate) => candidate.value === compressionLevel.value) || COMPRESSION_LEVELS[1];
});
const activeDropdownOptions = computed(() => (
  selectDropdown.value.key === 'compression' ? COMPRESSION_LEVELS : ARCHIVE_FORMATS
));
const activeDropdownValue = computed(() => (
  selectDropdown.value.key === 'compression' ? compressionLevel.value : format.value
));

function formatExtension(value) {
  return (ARCHIVE_FORMATS.find((candidate) => candidate.value === value) || ARCHIVE_FORMATS[0]).extension;
}

function stripArchiveExtension(value) {
  const trimmed = String(value || '').trim();
  const lower = trimmed.toLocaleLowerCase();
  const extensions = ARCHIVE_FORMATS
    .map((candidate) => candidate.extension)
    .sort((a, b) => b.length - a.length);
  const extension = extensions.find((candidate) => lower.endsWith(candidate));

  if (!extension) {
    return trimmed;
  }

  return trimmed.slice(0, -extension.length);
}

function safeArchiveStem(value) {
  return stripArchiveExtension(value)
    .replace(/[\\/]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .replace(/^\.+|\.+$/g, '') || 'Archive';
}

function normalizeArchiveName(value, archiveFormat) {
  const raw = String(value || '').trim();

  if (!raw || raw === '.' || raw === '..' || /[\\/]/.test(raw)) {
    return '';
  }

  const stem = safeArchiveStem(value);

  if (!stem || stem === '.' || stem === '..') {
    return '';
  }

  return `${stem}${formatExtension(archiveFormat)}`;
}

function defaultArchiveStem() {
  if (props.entries.length === 1) {
    return safeArchiveStem(props.entries[0]?.name || 'Archive');
  }

  return 'Archive';
}

function uniqueArchiveName(seed, archiveFormat) {
  const normalized = normalizeArchiveName(seed, archiveFormat);

  if (!existingNameSet.value.has(normalized.toLocaleLowerCase())) {
    return normalized;
  }

  const stem = stripArchiveExtension(normalized);
  const extension = formatExtension(archiveFormat);

  for (let index = 2; index < 1000; index += 1) {
    const candidate = `${stem} ${index}${extension}`;

    if (!existingNameSet.value.has(candidate.toLocaleLowerCase())) {
      return candidate;
    }
  }

  return normalized;
}

function resetDialog() {
  format.value = 'zip';
  compressionLevel.value = 'balanced';
  includeTopLevelDirectory.value = true;
  replaceExisting.value = false;
  usePassword.value = false;
  password.value = '';
  confirmPassword.value = '';
  archiveName.value = uniqueArchiveName(defaultArchiveStem(), format.value);

  nextTick(() => nameInput.value?.focus());
}

function selectTriggerFor(key) {
  return key === 'compression' ? compressionTriggerRef.value : formatTriggerRef.value;
}

function openSelectDropdown(key) {
  if (key === 'compression' && compressionDisabled.value) {
    return;
  }

  const trigger = selectTriggerFor(key);

  if (!trigger) {
    return;
  }

  const rect = trigger.getBoundingClientRect();
  selectDropdown.value = {
    key,
    top: rect.bottom + 5,
    left: rect.left,
    width: rect.width,
  };

  nextTick(() => {
    document.addEventListener('pointerdown', handleSelectOutsideClick, { capture: true });
  });
}

function closeSelectDropdown() {
  selectDropdown.value = {
    key: null,
    top: 0,
    left: 0,
    width: 0,
  };
  document.removeEventListener('pointerdown', handleSelectOutsideClick, { capture: true });
}

function toggleSelectDropdown(key) {
  if (selectDropdown.value.key === key) {
    closeSelectDropdown();
    return;
  }

  openSelectDropdown(key);
}

function chooseSelectOption(key, value) {
  if (key === 'compression') {
    compressionLevel.value = value;
  } else {
    format.value = value;
  }

  closeSelectDropdown();
}

function handleSelectOutsideClick(event) {
  if (
    !formatTriggerRef.value?.contains(event.target) &&
    !compressionTriggerRef.value?.contains(event.target) &&
    !selectDropdownRef.value?.contains(event.target)
  ) {
    closeSelectDropdown();
  }
}

function cancel() {
  closeSelectDropdown();
  emit('cancel');
}

function create() {
  if (createDisabled.value) {
    return;
  }

  emit('create', {
    archiveName: normalizedArchiveName.value,
    format: format.value,
    compressionLevel: compressionLevel.value,
    includeTopLevelDirectory: includeTopLevelDirectory.value,
    password: usePassword.value && passwordAvailable.value ? password.value : null,
    overwrite: nameExists.value && replaceExisting.value,
  });
}

function handleKeydown(event) {
  if (!props.visible) {
    return;
  }

  if (event.key === 'Escape') {
    event.preventDefault();
    if (selectDropdown.value.key) {
      closeSelectDropdown();
      return;
    }
    cancel();
  }
}

watch(
  () => props.visible,
  (visible) => {
    if (visible) {
      resetDialog();
    }
  },
);

watch(format, (nextFormat, previousFormat) => {
  const stem = stripArchiveExtension(archiveName.value) || defaultArchiveStem();

  archiveName.value = uniqueArchiveName(`${stem}${formatExtension(previousFormat)}`, nextFormat);
  replaceExisting.value = false;

  if (nextFormat !== 'zip') {
    usePassword.value = false;
    password.value = '';
    confirmPassword.value = '';
  }

  if (nextFormat === 'tar' && selectDropdown.value.key === 'compression') {
    closeSelectDropdown();
  }
});

watch(
  () => props.entries,
  () => {
    if (props.visible) {
      resetDialog();
    }
  },
);

onMounted(() => {
  window.addEventListener('keydown', handleKeydown);
});

onUnmounted(() => {
  closeSelectDropdown();
  window.removeEventListener('keydown', handleKeydown);
});
</script>

<template>
  <Teleport to="body">
    <Transition name="archive-dialog">
      <div v-if="visible" class="archive-overlay" @pointerdown.self="cancel">
        <section class="archive-panel" role="dialog" aria-modal="true" aria-labelledby="archive-title">
          <div class="archive-content">
            <header class="archive-header">
              <div class="archive-title-row">
                <span class="archive-icon" aria-hidden="true">
                  <AppIcon name="archive" :size="20" :stroke-width="1.8" />
                </span>
                <h2 id="archive-title">Create Archive</h2>
              </div>
              <p>{{ selectedLabel }} in {{ directory }}</p>
            </header>

            <label class="archive-field">
              <span>Name</span>
              <input
                ref="nameInput"
                v-model="archiveName"
                type="text"
                autocomplete="off"
                spellcheck="false"
                @keydown.enter.prevent="create"
              >
            </label>

            <div class="archive-select-grid">
              <div class="archive-select-field">
                <span>Format</span>
                <div ref="formatTriggerRef" class="archive-select">
                  <button
                    type="button"
                    class="archive-select-trigger"
                    :class="{ 'archive-select-trigger--open': selectDropdown.key === 'format' }"
                    aria-haspopup="listbox"
                    :aria-expanded="selectDropdown.key === 'format'"
                    @click="toggleSelectDropdown('format')"
                  >
                    <span class="archive-select-icon" aria-hidden="true">
                      <AppIcon :name="currentFormat.icon" :size="17" :stroke-width="1.8" />
                    </span>
                    <span class="archive-select-body">
                      <span class="archive-select-label">{{ currentFormat.label }}</span>
                      <span class="archive-select-desc">{{ currentFormat.description }}</span>
                    </span>
                    <span class="archive-select-chevron" aria-hidden="true">
                      <AppIcon name="chevron-down" :size="14" :stroke-width="2.2" />
                    </span>
                  </button>
                </div>
              </div>

              <div class="archive-select-field">
                <span>Compression</span>
                <div ref="compressionTriggerRef" class="archive-select">
                  <button
                    type="button"
                    class="archive-select-trigger"
                    :class="{ 'archive-select-trigger--open': selectDropdown.key === 'compression' }"
                    :disabled="compressionDisabled"
                    aria-haspopup="listbox"
                    :aria-expanded="selectDropdown.key === 'compression'"
                    @click="toggleSelectDropdown('compression')"
                  >
                    <span class="archive-select-icon" aria-hidden="true">
                      <AppIcon :name="currentCompression.icon" :size="17" :stroke-width="1.8" />
                    </span>
                    <span class="archive-select-body">
                      <span class="archive-select-label">{{ currentCompression.label }}</span>
                      <span class="archive-select-desc">{{ currentCompression.description }}</span>
                    </span>
                    <span class="archive-select-chevron" aria-hidden="true">
                      <AppIcon name="chevron-down" :size="14" :stroke-width="2.2" />
                    </span>
                  </button>
                </div>
              </div>
            </div>

            <Teleport to="body">
              <div
                v-if="selectDropdown.key"
                ref="selectDropdownRef"
                class="archive-select-dropdown"
                role="listbox"
                :style="{
                  top: `${selectDropdown.top}px`,
                  left: `${selectDropdown.left}px`,
                  width: `${selectDropdown.width}px`,
                }"
              >
                <button
                  v-for="candidate in activeDropdownOptions"
                  :key="candidate.value"
                  type="button"
                  class="archive-select-option"
                  :class="{ 'archive-select-option--active': activeDropdownValue === candidate.value }"
                  role="option"
                  :aria-selected="activeDropdownValue === candidate.value"
                  @click="chooseSelectOption(selectDropdown.key, candidate.value)"
                >
                  <span class="archive-select-option-icon" aria-hidden="true">
                    <AppIcon :name="candidate.icon" :size="16" :stroke-width="1.8" />
                  </span>
                  <span class="archive-select-option-body">
                    <span class="archive-select-option-label">{{ candidate.label }}</span>
                    <span class="archive-select-option-desc">{{ candidate.description }}</span>
                  </span>
                  <span
                    v-if="activeDropdownValue === candidate.value"
                    class="archive-select-option-check"
                    aria-hidden="true"
                  >
                    <AppIcon name="check" :size="13" :stroke-width="2.6" />
                  </span>
                </button>
              </div>
            </Teleport>

            <div class="archive-options">
              <label
                class="archive-switch-row"
                :class="{ 'archive-switch-row--disabled': !canToggleTopLevel }"
              >
                <span class="archive-switch-copy">
                  <strong>Include top-level folder</strong>
                  <span>Keep the selected folder as the archive root.</span>
                </span>
                <input
                  v-model="includeTopLevelDirectory"
                  class="switch-input"
                  type="checkbox"
                  :disabled="!canToggleTopLevel"
                >
                <span class="settings-switch" aria-hidden="true"></span>
              </label>

              <label
                class="archive-switch-row"
                :class="{ 'archive-switch-row--disabled': !passwordAvailable }"
              >
                <span class="archive-switch-copy">
                  <strong>Password protect ZIP</strong>
                  <span>Available for ZIP archives only.</span>
                </span>
                <input
                  v-model="usePassword"
                  class="switch-input"
                  type="checkbox"
                  :disabled="!passwordAvailable"
                >
                <span class="settings-switch" aria-hidden="true"></span>
              </label>
            </div>

            <div v-if="usePassword && passwordAvailable" class="archive-password-grid">
              <label class="archive-field">
                <span>Password</span>
                <input v-model="password" type="password" autocomplete="new-password">
              </label>
              <label class="archive-field">
                <span>Confirm</span>
                <input v-model="confirmPassword" type="password" autocomplete="new-password">
              </label>
              <p class="archive-note">
                ZIP passwords use legacy ZipCrypto compatibility.
              </p>
            </div>

            <p v-else-if="!passwordAvailable" class="archive-note">
              Passwords are available only for ZIP archives.
            </p>

            <label v-if="nameExists" class="archive-switch-row archive-switch-row--replace">
              <span class="archive-switch-copy">
                <strong>Replace existing archive</strong>
                <span>{{ normalizedArchiveName }}</span>
              </span>
              <input v-model="replaceExisting" class="switch-input" type="checkbox">
              <span class="settings-switch" aria-hidden="true"></span>
            </label>

            <p v-if="passwordMismatch" class="archive-error">Passwords do not match.</p>

            <footer class="archive-actions">
              <button type="button" class="archive-button" @click="cancel">Cancel</button>
              <button
                type="button"
                class="archive-button archive-button--primary"
                :disabled="createDisabled"
                @click="create"
              >
                Create {{ currentFormat.label }}
              </button>
            </footer>
          </div>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.archive-overlay {
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

.archive-panel {
  display: block;
  width: min(540px, calc(100vw - 48px));
  max-height: calc(100vh - 56px);
  overflow-x: hidden;
  overflow-y: auto;
  border: 1px solid var(--control-border);
  border-radius: 11px;
  padding: 16px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
}

.archive-icon {
  display: grid;
  width: 24px;
  height: 24px;
  flex: 0 0 auto;
  place-items: center;
  color: var(--icon);
}

.archive-content {
  display: grid;
  min-width: 0;
  gap: 13px;
}

.archive-header {
  display: grid;
  gap: 5px;
}

.archive-title-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 9px;
}

.archive-header h2 {
  margin: 0;
  color: var(--text);
  font-size: 14.5px;
  font-weight: 700;
  letter-spacing: 0;
}

.archive-header p {
  overflow: hidden;
  margin: 0;
  color: var(--text-muted);
  font-size: 12.5px;
  font-weight: 500;
  line-height: 1.35;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.archive-field,
.archive-select-field {
  display: grid;
  gap: 7px;
  min-width: 0;
  margin: 0;
  padding: 0;
}

.archive-field > span,
.archive-select-field > span {
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 700;
}

.archive-field input[type="text"],
.archive-field input[type="password"] {
  width: 100%;
  height: 34px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 0 11px;
  background: var(--input-bg);
  color: var(--text);
  font-size: 13px;
  font-weight: 520;
  outline: 0;
  box-shadow: var(--input-shadow);
}

.archive-field input:focus {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.archive-select-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 9px;
  min-width: 0;
}

.archive-select {
  position: relative;
}

.archive-select-trigger {
  display: flex;
  width: 100%;
  min-width: 0;
  height: 48px;
  align-items: center;
  gap: 10px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 0 11px 0 13px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text);
  text-align: left;
  transition: border-color 120ms ease, box-shadow 120ms ease, opacity 120ms ease;
}

.archive-select-trigger:hover:not(:disabled) {
  border-color: var(--control-border);
}

.archive-select-trigger:disabled {
  cursor: default;
  opacity: 0.62;
}

.archive-select-trigger--open {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.archive-select-icon,
.archive-select-chevron,
.archive-select-option-icon,
.archive-select-option-check {
  display: flex;
  flex-shrink: 0;
}

.archive-select-icon {
  color: var(--accent);
}

.archive-select-body {
  display: flex;
  flex: 1;
  min-width: 0;
  flex-direction: column;
  gap: 2px;
}

.archive-select-label {
  overflow: hidden;
  font-size: 13px;
  font-weight: 650;
  line-height: 1;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.archive-select-desc {
  overflow: hidden;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
  line-height: 1;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.archive-select-chevron {
  color: var(--icon);
  transition: transform 160ms cubic-bezier(0.2, 0, 0, 1);
}

.archive-select-trigger--open .archive-select-chevron {
  transform: rotate(180deg);
}

.archive-select-dropdown {
  position: fixed;
  z-index: 9000;
  overflow-y: auto;
  max-height: 320px;
  padding: 4px;
  border: 1px solid var(--control-border);
  border-radius: 11px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  animation: archive-select-dropdown-in 130ms cubic-bezier(0.2, 0, 0, 1) forwards;
}

@keyframes archive-select-dropdown-in {
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

.archive-select-option {
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

.archive-select-option:hover {
  background: var(--btn-hover);
}

.archive-select-option--active {
  background: rgb(var(--accent-rgb) / 0.08);
}

.archive-select-option-icon {
  width: 30px;
  height: 30px;
  align-items: center;
  justify-content: center;
  border-radius: 8px;
  background: rgb(var(--accent-rgb) / 0.10);
  color: var(--accent);
}

.archive-select-option--active .archive-select-option-icon {
  background: rgb(var(--accent-rgb) / 0.16);
}

.archive-select-option-body {
  display: flex;
  flex: 1;
  min-width: 0;
  flex-direction: column;
  gap: 2px;
}

.archive-select-option-label {
  font-size: 13px;
  font-weight: 580;
  line-height: 1;
}

.archive-select-option-desc {
  overflow: hidden;
  color: var(--text-faint);
  font-size: 11px;
  line-height: 1;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.archive-select-option-check {
  color: var(--accent);
}

.archive-options {
  display: grid;
  gap: 8px;
}

.archive-switch-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  min-width: 0;
  min-height: 50px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 9px 10px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text-muted);
}

.archive-switch-row--disabled {
  opacity: 0.58;
}

.archive-switch-copy {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.archive-switch-copy strong {
  overflow: hidden;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 700;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.archive-switch-copy span {
  overflow: hidden;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.switch-input {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  clip-path: inset(50%);
  white-space: nowrap;
}

.settings-switch {
  position: relative;
  display: block;
  width: 42px;
  height: 24px;
  flex: 0 0 42px;
  border: 1px solid var(--input-border);
  border-radius: 999px;
  background: color-mix(in srgb, var(--text) 9%, transparent);
  box-shadow: var(--input-shadow);
  transition: background 120ms ease, border-color 120ms ease;
}

.settings-switch::after {
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

.switch-input:checked + .settings-switch {
  border-color: var(--accent-border);
  background: var(--accent);
}

.switch-input:checked + .settings-switch::after {
  background: #ffffff;
  transform: translateX(18px);
}

.switch-input:focus-visible + .settings-switch {
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.archive-password-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
}

.archive-note,
.archive-error {
  margin: 0;
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 600;
  line-height: 1.35;
}

.archive-password-grid .archive-note {
  grid-column: 1 / -1;
}

.archive-error {
  color: var(--danger);
}

.archive-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  padding-top: 2px;
}

.archive-button {
  min-width: 86px;
  height: 36px;
  border: 1px solid color-mix(in srgb, var(--text) 13%, transparent);
  border-radius: 999px;
  padding: 0 18px;
  background:
    linear-gradient(180deg, rgb(255 255 255 / 0.14), rgb(255 255 255 / 0.04)),
    color-mix(in srgb, var(--control-glass) 72%, transparent);
  color: var(--text);
  font-size: 13px;
  font-weight: 650;
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.16),
    inset 0 -1px 0 rgb(0 0 0 / 0.22),
    0 1px 2px rgb(0 0 0 / 0.24);
  transition:
    background 100ms ease,
    border-color 100ms ease,
    box-shadow 100ms ease,
    transform 80ms ease;
}

.archive-button:hover:not(:disabled) {
  border-color: color-mix(in srgb, var(--text) 20%, transparent);
  background:
    linear-gradient(180deg, rgb(255 255 255 / 0.18), rgb(255 255 255 / 0.06)),
    color-mix(in srgb, var(--control-glass) 82%, transparent);
}

.archive-button:active:not(:disabled) {
  transform: translateY(1px);
  box-shadow:
    inset 0 1px 2px rgb(0 0 0 / 0.22),
    0 1px 1px rgb(0 0 0 / 0.18);
}

.archive-button:disabled {
  opacity: 0.55;
  cursor: default;
}

.archive-button--primary {
  border-color: rgb(var(--accent-rgb) / 0.58);
  background:
    linear-gradient(180deg, rgb(72 176 255), rgb(0 113 242));
  color: rgb(255 255 255 / 0.96);
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.34),
    inset 0 -1px 0 rgb(0 48 120 / 0.35),
    0 0 0 1px rgb(var(--accent-rgb) / 0.18),
    0 4px 14px rgb(var(--accent-rgb) / 0.32);
}

.archive-button--primary:hover:not(:disabled) {
  background:
    linear-gradient(180deg, rgb(91 188 255), rgb(0 123 255));
}

.archive-dialog-enter-active,
.archive-dialog-leave-active {
  transition: opacity 120ms ease;
}

.archive-dialog-enter-active .archive-panel,
.archive-dialog-leave-active .archive-panel {
  transition: transform 120ms ease, opacity 120ms ease;
}

.archive-dialog-enter-from,
.archive-dialog-leave-to {
  opacity: 0;
}

.archive-dialog-enter-from .archive-panel,
.archive-dialog-leave-to .archive-panel {
  opacity: 0;
  transform: translateY(6px) scale(0.985);
}

@media (max-width: 560px) {
  .archive-panel {
    padding: 14px;
  }

  .archive-select-grid,
  .archive-options,
  .archive-password-grid {
    grid-template-columns: 1fr;
  }
}

@media (prefers-reduced-motion: reduce) {
  .archive-dialog-enter-active,
  .archive-dialog-leave-active,
  .archive-dialog-enter-active .archive-panel,
  .archive-dialog-leave-active .archive-panel {
    transition: none;
  }
}
</style>
