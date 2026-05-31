<script setup>
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { useScrollableContentState } from '../composables/useScrollableContentState';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { COLOR_SCHEME_OPTIONS, normalizeAccentColor } from '../utils/colorSchemes';
import { DATE_FORMAT_OPTIONS, formatDate } from '../utils/dateFormat';
import tauriConfig from '../../src-tauri/tauri.conf.json';
import appIconUrl from '../../src-tauri/icons/128x128.png';

const store = useFileManagerStore();

const searchQuery = ref('');
const activeSectionId = ref('appearance');
const settingsContent = ref(null);
const editorTemplatesVisible = ref(false);
const editorTemplateControl = ref(null);
const updateState = ref('idle');
const updateMessage = ref('');
const updateError = ref('');
const updateDetails = ref(null);
const updateProgress = ref(null);
let pendingUpdate = null;

const appInfo = {
  name: tauriConfig.productName || 'Carelo',
  version: tauriConfig.version || '',
  identifier: tauriConfig.identifier || '',
  description: tauriConfig.bundle?.longDescription || tauriConfig.bundle?.shortDescription || '',
  publisher: tauriConfig.bundle?.publisher || '',
  license: tauriConfig.bundle?.license || '',
  copyright: tauriConfig.bundle?.copyright || '',
  updateEndpoint: tauriConfig.plugins?.updater?.endpoints?.[0] || '',
};

const sections = [
  {
    id: 'appearance',
    label: 'Appearance',
    icon: 'monitor',
    keywords: 'appearance theme color scheme accent color dark light auto system material one dark pro tokyo night',
  },
  {
    id: 'files',
    label: 'Files',
    icon: 'folder',
    keywords: 'files hidden default view list grid columns date format modified zebra alternate row background',
  },
  {
    id: 'startup',
    label: 'Startup',
    icon: 'sync',
    keywords: 'startup session restore tabs folders terminal',
  },
  {
    id: 'safety',
    label: 'Safety',
    icon: 'shield',
    keywords: 'safety confirm delete remove destructive trash permanent hard delete',
  },
  {
    id: 'tools',
    label: 'Tools',
    icon: 'terminal',
    keywords: 'tools context menu right click command code editor path external edit file preferred vscode vscodium cursor zed sublime kate neovim',
  },
  {
    id: 'terminal',
    label: 'Terminal',
    icon: 'terminal',
    keywords: 'terminal shell cwd active folder directory',
  },
  {
    id: 'about',
    label: 'About',
    icon: 'info',
    keywords: 'about version info license copyright publisher app carelo update updater github releases',
  },
];

const appearanceModes = [
  { value: 'system', label: 'Auto', icon: 'monitor' },
  { value: 'light', label: 'Light', icon: 'sun' },
  { value: 'dark', label: 'Dark', icon: 'moon' },
];
const colorSchemeOptions = COLOR_SCHEME_OPTIONS;
const accentPresets = [
  { value: '#0a84ff', label: 'Blue' },
  { value: '#30d158', label: 'Green' },
  { value: '#ff9f0a', label: 'Amber' },
  { value: '#ff453a', label: 'Red' },
  { value: '#bf5af2', label: 'Violet' },
  { value: '#64d2ff', label: 'Cyan' },
];

const viewModes = [
  { value: 'list', label: 'List', icon: 'list' },
  { value: 'grid', label: 'Grid', icon: 'grid' },
  { value: 'columns', label: 'Columns', icon: 'columns' },
];
const deletionModes = [
  { value: 'trash', label: 'Move to Trash', icon: 'trash' },
  { value: 'permanent', label: 'Delete permanently', icon: 'alert' },
];
const toolTargets = [
  { value: 'both', label: 'Both' },
  { value: 'files', label: 'Files' },
  { value: 'folders', label: 'Folders' },
];
const editorTemplates = [
  { id: 'system-default', label: 'System default', command: '' },
  { id: 'vscode', label: 'Visual Studio Code', command: 'code --reuse-window %path%' },
  { id: 'vscodium', label: 'VSCodium', command: 'codium --reuse-window %path%' },
  { id: 'cursor', label: 'Cursor', command: 'cursor --reuse-window %path%' },
  { id: 'zed', label: 'Zed', command: 'zed %path%' },
  { id: 'sublime-text', label: 'Sublime Text', command: 'subl %path%' },
  { id: 'kate', label: 'Kate', command: 'kate %path%' },
  { id: 'gnome-text-editor', label: 'GNOME Text Editor', command: 'gnome-text-editor %path%' },
  { id: 'neovim', label: 'Neovim', command: 'x-terminal-emulator -e nvim %path%' },
];

const activeSection = computed(() =>
  sections.find((section) => section.id === activeSectionId.value) || sections[0],
);
const { isScrollable: settingsContentScrollable } = useScrollableContentState(settingsContent, {
  watch: [
    () => store.settingsVisible,
    activeSectionId,
    searchQuery,
    editorTemplatesVisible,
    updateDetails,
    updateMessage,
  ],
});
const dateFormatSample = new Date(2026, 4, 18, 14, 30);
const updateProgressLabel = computed(() => {
  const progress = updateProgress.value;

  if (!progress) {
    return '';
  }

  if (progress.contentLength > 0) {
    const percent = Math.min(100, Math.round((progress.downloaded / progress.contentLength) * 100));
    return `${percent}%`;
  }

  return formatBytes(progress.downloaded);
});
const updateProgressWidth = computed(() => {
  const progress = updateProgress.value;

  if (!progress || progress.contentLength <= 0) {
    return '0%';
  }

  const percent = Math.min(100, Math.round((progress.downloaded / progress.contentLength) * 100));
  return `${percent}%`;
});
const updateActionLabel = computed(() => {
  if (updateState.value === 'checking') {
    return 'Checking...';
  }

  if (updateState.value === 'available') {
    return 'Install update';
  }

  if (updateState.value === 'downloading') {
    return updateProgressLabel.value ? `Downloading ${updateProgressLabel.value}` : 'Downloading...';
  }

  if (updateState.value === 'installed') {
    return 'Restarting...';
  }

  return 'Check for updates';
});
const updateActionIcon = computed(() => (updateState.value === 'available' ? 'download' : 'refresh'));
const updateActionDisabled = computed(() =>
  ['checking', 'downloading', 'installed'].includes(updateState.value),
);
const selectedEditorTemplate = computed(() => {
  const currentCommand = String(store.appSettings.editorCommand || '').trim();
  const template = editorTemplates.find((option) => option.command === currentCommand);

  return template?.id || 'custom';
});
const selectedColorScheme = computed(() =>
  colorSchemeOptions.find((option) => option.value === store.appSettings.colorScheme) || colorSchemeOptions[0],
);
const defaultAccentColor = computed(() => selectedColorScheme.value?.preview?.accent || '#0a84ff');
const customAccentColor = computed(() => normalizeAccentColor(store.appSettings.accentColor));
const activeAccentColor = computed(() => customAccentColor.value || defaultAccentColor.value);
const activeAccentLabel = computed(() => activeAccentColor.value.toUpperCase());
const presetAccentSelected = computed(() =>
  accentPresets.some((preset) => preset.value === customAccentColor.value),
);
const customAccentSelected = computed(() => Boolean(customAccentColor.value && !presetAccentSelected.value));

const visibleSections = computed(() => {
  const query = searchQuery.value.trim().toLowerCase();

  if (!query) {
    return sections;
  }

  return sections.filter((section) =>
    `${section.label} ${section.keywords}`.toLowerCase().includes(query),
  );
});

function setAppearanceMode(mode) {
  store.setAppSetting('appearanceMode', mode);
}

function setColorScheme(scheme) {
  store.setAppSetting('colorScheme', scheme);
}

function setAccentColor(color) {
  store.setAppSetting('accentColor', normalizeAccentColor(color));
}

function resetAccentColor() {
  store.setAppSetting('accentColor', '');
}

function colorSchemePreviewStyle(option) {
  return {
    '--scheme-sidebar': option.preview.sidebar,
    '--scheme-toolbar': option.preview.toolbar,
    '--scheme-pane': option.preview.pane,
    '--scheme-accent': option.preview.accent,
  };
}

function accentPresetStyle(color) {
  return {
    '--accent-choice': color,
  };
}

function setDefaultViewMode(viewMode) {
  store.setAppSetting('defaultViewMode', viewMode);
}

function setDeleteMode(deleteMode) {
  store.setAppSetting('deleteMode', deleteMode);
}

function dateFormatOptionPreview(format) {
  return formatDate(dateFormatSample, format, { includeTime: true });
}

function setDateFormat(format) {
  store.setAppSetting('dateFormat', format);
}

function setBooleanSetting(key, event) {
  store.setAppSetting(key, event.target.checked);
}

function setEditorTemplate(template) {
  store.setAppSetting('editorCommand', template.command);
  editorTemplatesVisible.value = false;
}

function toggleEditorTemplates() {
  editorTemplatesVisible.value = !editorTemplatesVisible.value;
}

function closeEditorTemplates() {
  editorTemplatesVisible.value = false;
}

function handleDocumentPointerDown(event) {
  if (!editorTemplatesVisible.value || editorTemplateControl.value?.contains(event.target)) {
    return;
  }

  closeEditorTemplates();
}

function formatBytes(value) {
  if (!Number.isFinite(value) || value <= 0) {
    return '0 B';
  }

  const units = ['B', 'KB', 'MB', 'GB'];
  let size = value;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }

  const precision = unitIndex === 0 || size >= 10 ? 0 : 1;
  return `${size.toFixed(precision)} ${units[unitIndex]}`;
}

function formatUpdateDate(value) {
  if (!value) {
    return '';
  }

  const date = new Date(value);

  if (Number.isNaN(date.getTime())) {
    return '';
  }

  return formatDate(date, store.appSettings.dateFormat, { includeTime: true });
}

function formatUpdateError(error) {
  const message = error instanceof Error ? error.message : String(error || '');

  if (/404|not found|release json|latest\.json/i.test(message)) {
    return 'No GitHub update metadata has been published yet.';
  }

  return message || 'The update check failed.';
}

async function checkForUpdates() {
  pendingUpdate = null;
  updateState.value = 'checking';
  updateMessage.value = '';
  updateError.value = '';
  updateDetails.value = null;
  updateProgress.value = null;

  try {
    const update = await check();

    if (!update) {
      updateState.value = 'current';
      updateMessage.value = 'Carelo is up to date.';
      return;
    }

    pendingUpdate = update;
    updateDetails.value = {
      version: update.version || '',
      date: update.date || '',
      body: update.body || '',
    };
    updateState.value = 'available';
    updateMessage.value = `Version ${update.version} is available.`;
  } catch (error) {
    updateState.value = 'error';
    updateError.value = formatUpdateError(error);
  }
}

async function installUpdate() {
  if (!pendingUpdate) {
    await checkForUpdates();
    return;
  }

  let downloaded = 0;
  let contentLength = 0;
  updateState.value = 'downloading';
  updateMessage.value = 'Downloading update...';
  updateError.value = '';
  updateProgress.value = { downloaded, contentLength };

  try {
    await pendingUpdate.downloadAndInstall((event) => {
      if (event.event === 'Started') {
        contentLength = Number(event.data?.contentLength || 0);
        downloaded = 0;
      } else if (event.event === 'Progress') {
        downloaded += Number(event.data?.chunkLength || 0);
      } else if (event.event === 'Finished') {
        downloaded = contentLength || downloaded;
      }

      updateProgress.value = { downloaded, contentLength };
    });

    pendingUpdate = null;
    updateState.value = 'installed';
    updateMessage.value = 'Update installed. Restarting Carelo...';

    try {
      await relaunch();
    } catch (error) {
      updateMessage.value = 'Update installed. Restart Carelo to finish.';
      updateError.value = formatUpdateError(error);
    }
  } catch (error) {
    updateState.value = 'error';
    updateMessage.value = '';
    updateError.value = formatUpdateError(error);
  }
}

function handleUpdateAction() {
  if (updateState.value === 'available') {
    installUpdate();
    return;
  }

  checkForUpdates();
}

function createToolId() {
  return `tool-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
}

function addCustomTool() {
  store.setAppSetting('customTools', [
    ...(store.appSettings.customTools || []),
    {
      id: createToolId(),
      name: '',
      command: '',
      enabled: true,
      appliesTo: 'both',
      extensions: '',
    },
  ]);
}

function updateCustomTool(id, patch) {
  store.setAppSetting(
    'customTools',
    (store.appSettings.customTools || []).map((tool) =>
      tool.id === id ? { ...tool, ...patch } : tool,
    ),
  );
}

function removeCustomTool(id) {
  store.setAppSetting(
    'customTools',
    (store.appSettings.customTools || []).filter((tool) => tool.id !== id),
  );
}

function close() {
  store.closeSettings();
}

function onKeydown(event) {
  if (event.key !== 'Escape' || !store.settingsVisible) {
    return;
  }

  if (editorTemplatesVisible.value) {
    closeEditorTemplates();
  } else {
    close();
  }
}

watch(visibleSections, (nextSections) => {
  if (nextSections.length > 0 && !nextSections.some((section) => section.id === activeSectionId.value)) {
    activeSectionId.value = nextSections[0].id;
  }
});

watch(
  () => store.settingsVisible,
  (visible) => {
    if (visible) {
      activeSectionId.value = activeSectionId.value || 'appearance';
    } else {
      closeEditorTemplates();
    }
  },
);

watch(
  () => activeSectionId.value,
  () => closeEditorTemplates(),
);

onMounted(() => {
  window.addEventListener('keydown', onKeydown);
  document.addEventListener('pointerdown', handleDocumentPointerDown, true);
});

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown);
  document.removeEventListener('pointerdown', handleDocumentPointerDown, true);
});
</script>

<template>
  <Teleport to="body">
    <Transition name="settings-fade">
      <div
        v-if="store.settingsVisible"
        class="settings-overlay"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        @pointerdown.self="close"
      >
        <section class="settings-window" @pointerdown.stop>
          <aside class="settings-sidebar" aria-label="Settings sections">
            <label class="settings-search">
              <AppIcon name="search" :size="14" />
              <input v-model="searchQuery" type="search" placeholder="Search" />
            </label>

            <nav class="settings-nav">
              <button
                v-for="section in visibleSections"
                :key="section.id"
                type="button"
                class="settings-nav-item"
                :class="{ active: activeSectionId === section.id }"
                @click="activeSectionId = section.id"
              >
                <AppIcon :name="section.icon" :size="18" :stroke-width="1.8" />
                <span>{{ section.label }}</span>
              </button>
            </nav>
          </aside>

          <main
            class="settings-main"
            :class="{ 'settings-main--content-scrollable': settingsContentScrollable }"
          >
            <header class="settings-header">
              <div class="settings-title-group">
                <div class="settings-header-icon">
                  <AppIcon :name="activeSection.icon" :size="19" :stroke-width="1.8" />
                </div>
                <h2 id="settings-title">{{ activeSection.label }}</h2>
              </div>

              <button type="button" class="settings-close" aria-label="Close settings" @click="close">
                <AppIcon name="x" :size="14" :stroke-width="2" />
              </button>
            </header>

            <div ref="settingsContent" class="settings-content">
              <section v-if="activeSectionId === 'appearance'" class="settings-page">
                <div class="settings-section-heading">
                  <h3>Appearance</h3>
                  <p>Choose the mode, theme, and highlight color Carelo uses.</p>
                </div>

                <div class="settings-group">
                  <div class="setting-row setting-row--stacked">
                    <div class="setting-copy">
                      <strong>App appearance</strong>
                      <span>Use the system setting, or force Carelo to stay light or dark.</span>
                    </div>

                    <div class="appearance-options" role="group" aria-label="App appearance">
                      <button
                        v-for="mode in appearanceModes"
                        :key="mode.value"
                        type="button"
                        class="appearance-option"
                        :class="[
                          `appearance-option--${mode.value}`,
                          { active: store.appSettings.appearanceMode === mode.value },
                        ]"
                        :aria-pressed="store.appSettings.appearanceMode === mode.value"
                        @click="setAppearanceMode(mode.value)"
                      >
                        <span class="appearance-preview" aria-hidden="true">
                          <span class="preview-sidebar"></span>
                          <span class="preview-toolbar"></span>
                          <span class="preview-pane"></span>
                          <span class="preview-accent"></span>
                        </span>
                        <span class="appearance-label">
                          <AppIcon :name="mode.icon" :size="14" :stroke-width="1.8" />
                          {{ mode.label }}
                        </span>
                      </button>
                    </div>
                  </div>

                  <div class="setting-row setting-row--stacked">
                    <div class="setting-copy">
                      <strong>Color theme</strong>
                      <span>Sets the palette Carelo uses in light, dark, and system appearance.</span>
                    </div>

                    <div class="scheme-options" role="group" aria-label="Color theme">
                      <button
                        v-for="scheme in colorSchemeOptions"
                        :key="scheme.value"
                        type="button"
                        class="scheme-option"
                        :class="{ active: store.appSettings.colorScheme === scheme.value }"
                        :style="colorSchemePreviewStyle(scheme)"
                        :aria-pressed="store.appSettings.colorScheme === scheme.value"
                        @click="setColorScheme(scheme.value)"
                      >
                        <span class="scheme-preview" aria-hidden="true">
                          <span class="scheme-preview-sidebar"></span>
                          <span class="scheme-preview-toolbar"></span>
                          <span class="scheme-preview-pane"></span>
                          <span class="scheme-preview-accent"></span>
                        </span>
                        <span class="scheme-details">
                          <span class="scheme-name">{{ scheme.label }}</span>
                          <span class="scheme-description">{{ scheme.description }}</span>
                        </span>
                        <span class="scheme-swatches" aria-hidden="true">
                          <span
                            v-for="swatch in scheme.swatches"
                            :key="`${scheme.value}-${swatch}`"
                            :style="{ background: swatch }"
                          ></span>
                        </span>
                      </button>
                    </div>
                  </div>

                  <div class="setting-row setting-row--stacked">
                    <div class="setting-copy">
                      <strong>Accent color</strong>
                      <span>Sets the color for selections, focus rings, folders, and primary actions.</span>
                    </div>

                    <div class="accent-color-control">
                      <div class="accent-color-grid" role="group" aria-label="Accent color">
                        <button
                          type="button"
                          class="accent-color-option accent-color-option--default"
                          :class="{ active: !customAccentColor }"
                          :style="accentPresetStyle(defaultAccentColor)"
                          :aria-pressed="!customAccentColor"
                          @click="resetAccentColor"
                        >
                          <span class="accent-color-swatch" aria-hidden="true"></span>
                          <span class="accent-color-option-copy">
                            <strong>Theme default</strong>
                            <span>{{ selectedColorScheme.label }}</span>
                          </span>
                        </button>

                        <button
                          v-for="preset in accentPresets"
                          :key="preset.value"
                          type="button"
                          class="accent-color-option"
                          :class="{ active: customAccentColor === preset.value }"
                          :style="accentPresetStyle(preset.value)"
                          :aria-pressed="customAccentColor === preset.value"
                          @click="setAccentColor(preset.value)"
                        >
                          <span class="accent-color-swatch" aria-hidden="true"></span>
                          <span class="accent-color-option-copy">
                            <strong>{{ preset.label }}</strong>
                            <span>{{ preset.value.toUpperCase() }}</span>
                          </span>
                        </button>
                      </div>

                      <label
                        class="accent-color-custom"
                        :class="{ active: customAccentSelected }"
                      >
                        <span class="accent-color-custom-copy">
                          <strong>Custom color</strong>
                          <span>{{ customAccentColor ? activeAccentLabel : 'Pick any color' }}</span>
                        </span>
                        <span class="accent-color-custom-picker">
                          <input
                            type="color"
                            :value="activeAccentColor"
                            aria-label="Choose custom accent color"
                            @input="setAccentColor($event.target.value)"
                          />
                          <span
                            class="accent-color-swatch"
                            :style="{ '--accent-choice': activeAccentColor }"
                            aria-hidden="true"
                          ></span>
                          <span>Choose</span>
                        </span>
                      </label>
                    </div>
                  </div>
                </div>
              </section>

              <section v-else-if="activeSectionId === 'files'" class="settings-page">
                <div class="settings-section-heading">
                  <h3>Files</h3>
                  <p>Set the default file browsing behavior for new panes and tabs.</p>
                </div>

                <div class="settings-group">
                  <div class="setting-row setting-row--stacked">
                    <div class="setting-copy">
                      <strong>Default view for new tabs</strong>
                      <span>Existing tabs keep their current view. New tabs use this mode.</span>
                    </div>

                    <div class="view-segment" role="group" aria-label="Default view mode">
                      <button
                        v-for="viewMode in viewModes"
                        :key="viewMode.value"
                        type="button"
                        :class="{ active: store.appSettings.defaultViewMode === viewMode.value }"
                        :aria-pressed="store.appSettings.defaultViewMode === viewMode.value"
                        @click="setDefaultViewMode(viewMode.value)"
                      >
                        <AppIcon :name="viewMode.icon" :size="16" :stroke-width="1.8" />
                        <span>{{ viewMode.label }}</span>
                      </button>
                    </div>
                  </div>

                  <div class="setting-row setting-row--stacked">
                    <div class="setting-copy">
                      <strong>Date format</strong>
                      <span>Used in file lists, preview metadata, and file conflict dialogs.</span>
                    </div>

                    <div class="date-format-options" role="group" aria-label="Date format">
                      <button
                        v-for="option in DATE_FORMAT_OPTIONS"
                        :key="option.value"
                        type="button"
                        class="date-format-option"
                        :class="{ active: store.appSettings.dateFormat === option.value }"
                        :aria-pressed="store.appSettings.dateFormat === option.value"
                        @click="setDateFormat(option.value)"
                      >
                        <span class="date-format-copy">
                          <strong>{{ option.label }}</strong>
                          <span>{{ dateFormatOptionPreview(option.value) }}</span>
                        </span>
                        <span
                          class="date-format-check"
                          :class="{ visible: store.appSettings.dateFormat === option.value }"
                          aria-hidden="true"
                        >
                          <AppIcon name="check" :size="14" :stroke-width="2.1" />
                        </span>
                      </button>
                    </div>
                  </div>

                  <label class="setting-row setting-row--switch">
                    <span class="setting-copy">
                      <strong>Show hidden files</strong>
                      <span>Display dotfiles and hidden filesystem entries in every pane.</span>
                    </span>
                    <input
                      class="switch-input"
                      type="checkbox"
                      :checked="store.showHiddenFiles"
                      @change="store.setShowHiddenFiles($event.target.checked)"
                    />
                    <span class="settings-switch" aria-hidden="true"></span>
                  </label>

                  <label class="setting-row setting-row--switch">
                    <span class="setting-copy">
                      <strong>Alternate row backgrounds</strong>
                      <span>Tint every other row in list and column view.</span>
                    </span>
                    <input
                      class="switch-input"
                      type="checkbox"
                      :checked="store.appSettings.alternateRowColors"
                      @change="setBooleanSetting('alternateRowColors', $event)"
                    />
                    <span class="settings-switch" aria-hidden="true"></span>
                  </label>
                </div>
              </section>

              <section v-else-if="activeSectionId === 'startup'" class="settings-page">
                <div class="settings-section-heading">
                  <h3>Startup</h3>
                  <p>Control what Carelo restores the next time it opens.</p>
                </div>

                <div class="settings-group">
                  <label class="setting-row setting-row--switch">
                    <span class="setting-copy">
                      <strong>Restore previous tabs and folders</strong>
                      <span>Reopen the same pane tabs, paths, sort order, and view modes.</span>
                    </span>
                    <input
                      class="switch-input"
                      type="checkbox"
                      :checked="store.appSettings.restoreSession"
                      @change="setBooleanSetting('restoreSession', $event)"
                    />
                    <span class="settings-switch" aria-hidden="true"></span>
                  </label>

                  <label class="setting-row setting-row--switch">
                    <span class="setting-copy">
                      <strong>Restore terminal panel</strong>
                      <span>Open the terminal panel on launch when it was visible last time.</span>
                    </span>
                    <input
                      class="switch-input"
                      type="checkbox"
                      :checked="store.appSettings.restoreTerminalPanel"
                      @change="setBooleanSetting('restoreTerminalPanel', $event)"
                    />
                    <span class="settings-switch" aria-hidden="true"></span>
                  </label>
                </div>
              </section>

              <section v-else-if="activeSectionId === 'safety'" class="settings-page">
                <div class="settings-section-heading">
                  <h3>Safety</h3>
                  <p>Keep destructive file actions explicit.</p>
                </div>

                <div class="settings-group">
                  <div class="setting-row setting-row--stacked">
                    <div class="setting-copy">
                      <strong>Deletion behavior</strong>
                      <span>Choose whether local deletes go to the system Trash or are removed immediately.</span>
                    </div>

                    <div class="view-segment delete-mode-segment" role="group" aria-label="Deletion behavior">
                      <button
                        v-for="mode in deletionModes"
                        :key="mode.value"
                        type="button"
                        :class="{ active: store.appSettings.deleteMode === mode.value }"
                        :aria-pressed="store.appSettings.deleteMode === mode.value"
                        @click="setDeleteMode(mode.value)"
                      >
                        <AppIcon :name="mode.icon" :size="16" :stroke-width="1.8" />
                        <span>{{ mode.label }}</span>
                      </button>
                    </div>
                  </div>

                  <label class="setting-row setting-row--switch">
                    <span class="setting-copy">
                      <strong>Confirm before deleting</strong>
                      <span>Ask before deleting files from toolbars, menus, and keyboard shortcuts.</span>
                    </span>
                    <input
                      class="switch-input"
                      type="checkbox"
                      :checked="store.appSettings.confirmDelete"
                      @change="setBooleanSetting('confirmDelete', $event)"
                    />
                    <span class="settings-switch" aria-hidden="true"></span>
                  </label>
                </div>
              </section>

              <section v-else-if="activeSectionId === 'tools'" class="settings-page">
                <div class="settings-section-heading">
                  <h3>Tools</h3>
                  <p>Choose your editor and add commands to the file context menu.</p>
                </div>

                <div class="settings-group settings-group--tools">
                  <div class="setting-row setting-row--stacked">
                    <div class="setting-copy">
                      <strong>Editor command</strong>
                      <span>Used for file editing. Leave empty to use the system default app.</span>
                    </div>

                    <div class="editor-command-field">
                      <label class="editor-command-input">
                        <span>Command</span>
                        <input
                          type="text"
                          :value="store.appSettings.editorCommand"
                          placeholder="code --reuse-window %path%"
                          spellcheck="false"
                          @input="store.setAppSetting('editorCommand', $event.target.value)"
                        />
                      </label>

                      <div ref="editorTemplateControl" class="editor-template-control">
                        <button
                          type="button"
                          class="editor-template-link"
                          :class="{ 'editor-template-link--open': editorTemplatesVisible }"
                          aria-haspopup="menu"
                          :aria-expanded="editorTemplatesVisible"
                          @click="toggleEditorTemplates"
                        >
                          Choose from common editors
                        </button>

                        <Transition name="editor-template-popover">
                          <div
                            v-if="editorTemplatesVisible"
                            class="editor-template-popover"
                            role="menu"
                            aria-label="Common editor commands"
                          >
                            <button
                              v-for="template in editorTemplates"
                              :key="template.id"
                              type="button"
                              role="menuitem"
                              class="editor-template-option"
                              :class="{ 'editor-template-option--active': selectedEditorTemplate === template.id }"
                              @click="setEditorTemplate(template)"
                            >
                              <span>
                                <strong>{{ template.label }}</strong>
                                <small>{{ template.command || 'Use the system default app' }}</small>
                              </span>
                              <AppIcon
                                v-if="selectedEditorTemplate === template.id"
                                name="check"
                                :size="14"
                                :stroke-width="2.2"
                              />
                            </button>
                          </div>
                        </Transition>
                      </div>
                    </div>
                  </div>

                  <div class="setting-row setting-row--stacked">
                    <div class="setting-copy">
                      <strong>Context menu tools</strong>
                      <span>Use placeholders such as %path%, %paths%, %name%, and %parent%.</span>
                    </div>

                    <div v-if="store.appSettings.customTools.length > 0" class="custom-tool-list">
                      <article
                        v-for="tool in store.appSettings.customTools"
                        :key="tool.id"
                        class="custom-tool-card"
                        :class="{ 'custom-tool-card--disabled': !tool.enabled }"
                      >
                        <div class="custom-tool-header">
                          <label class="custom-tool-field custom-tool-field--name">
                            <span>Name</span>
                            <input
                              type="text"
                              :value="tool.name"
                              placeholder="Open in Code"
                              @input="updateCustomTool(tool.id, { name: $event.target.value })"
                            />
                          </label>

                          <label class="custom-tool-toggle">
                            <input
                              class="switch-input"
                              type="checkbox"
                              :checked="tool.enabled"
                              @change="updateCustomTool(tool.id, { enabled: $event.target.checked })"
                            />
                            <span class="settings-switch" aria-hidden="true"></span>
                          </label>

                          <button
                            type="button"
                            class="custom-tool-remove"
                            aria-label="Remove tool"
                            @click="removeCustomTool(tool.id)"
                          >
                            <AppIcon name="x" :size="13" :stroke-width="2" />
                          </button>
                        </div>

                        <label class="custom-tool-field">
                          <span>Command</span>
                          <input
                            type="text"
                            :value="tool.command"
                            placeholder="code %path%"
                            spellcheck="false"
                            @input="updateCustomTool(tool.id, { command: $event.target.value })"
                          />
                        </label>

                        <div class="custom-tool-scope">
                          <span>Available for</span>
                          <div class="custom-tool-targets" role="group" aria-label="Tool availability">
                            <button
                              v-for="target in toolTargets"
                              :key="`${tool.id}-${target.value}`"
                              type="button"
                              :class="{ active: (tool.appliesTo || 'both') === target.value }"
                              :aria-pressed="(tool.appliesTo || 'both') === target.value"
                              @click="updateCustomTool(tool.id, { appliesTo: target.value })"
                            >
                              {{ target.label }}
                            </button>
                          </div>
                        </div>

                        <label v-if="tool.appliesTo === 'files'" class="custom-tool-field">
                          <span>File extensions</span>
                          <input
                            type="text"
                            :value="tool.extensions"
                            placeholder="js, ts, vue"
                            spellcheck="false"
                            @input="updateCustomTool(tool.id, { extensions: $event.target.value })"
                          />
                        </label>
                      </article>
                    </div>

                    <div v-else class="custom-tool-empty">
                      <AppIcon name="terminal" :size="18" :stroke-width="1.8" />
                      <span>No tools configured</span>
                    </div>

                    <button type="button" class="custom-tool-add" @click="addCustomTool">
                      <AppIcon name="plus" :size="15" :stroke-width="2" />
                      <span>Add tool</span>
                    </button>
                  </div>
                </div>
              </section>

              <section v-else-if="activeSectionId === 'terminal'" class="settings-page">
                <div class="settings-section-heading">
                  <h3>Terminal</h3>
                  <p>Choose where new terminal sessions start.</p>
                </div>

                <div class="settings-group">
                  <label class="setting-row setting-row--switch">
                    <span class="setting-copy">
                      <strong>Start in active folder</strong>
                      <span>Use the active pane path, including the active column in column view.</span>
                    </span>
                    <input
                      class="switch-input"
                      type="checkbox"
                      :checked="store.appSettings.terminalStartsInActiveFolder"
                      @change="setBooleanSetting('terminalStartsInActiveFolder', $event)"
                    />
                    <span class="settings-switch" aria-hidden="true"></span>
                  </label>
                </div>
              </section>

              <section v-else-if="activeSectionId === 'about'" class="settings-page">
                <div class="settings-section-heading">
                  <h3>About</h3>
                  <p>Application details and build metadata.</p>
                </div>

                <div class="settings-group about-group">
                  <div class="about-summary">
                    <span class="about-app-mark" aria-hidden="true">
                      <img :src="appIconUrl" alt="" />
                    </span>
                    <div class="about-copy">
                      <strong>{{ appInfo.name }}</strong>
                      <span>{{ appInfo.description }}</span>
                    </div>
                  </div>

                  <dl class="about-details">
                    <div>
                      <dt>Version</dt>
                      <dd>{{ appInfo.version }}</dd>
                    </div>
                    <div>
                      <dt>Publisher</dt>
                      <dd>{{ appInfo.publisher }}</dd>
                    </div>
                    <div>
                      <dt>License</dt>
                      <dd>{{ appInfo.license }}</dd>
                    </div>
                    <div v-if="appInfo.copyright">
                      <dt>Copyright</dt>
                      <dd>{{ appInfo.copyright }}</dd>
                    </div>
                  </dl>

                  <div class="about-update" aria-live="polite">
                    <div class="about-update-main">
                      <span class="about-update-copy">
                        <strong>Updates</strong>
                        <span v-if="updateError" class="about-update-error">{{ updateError }}</span>
                        <span v-else-if="updateMessage">{{ updateMessage }}</span>
                        <span v-else>Check whether a newer version is available.</span>
                      </span>

                      <button
                        type="button"
                        class="about-update-button"
                        :disabled="updateActionDisabled"
                        @click="handleUpdateAction"
                      >
                        <AppIcon :name="updateActionIcon" :size="15" :stroke-width="2" />
                        <span>{{ updateActionLabel }}</span>
                      </button>
                    </div>

                    <div v-if="updateState === 'downloading'" class="about-update-progress">
                      <span :style="{ width: updateProgressWidth }"></span>
                    </div>

                    <div v-if="updateDetails" class="about-update-release">
                      <span>
                        <strong>{{ updateDetails.version }}</strong>
                        <template v-if="formatUpdateDate(updateDetails.date)">
                          - {{ formatUpdateDate(updateDetails.date) }}
                        </template>
                      </span>
                      <p v-if="updateDetails.body">{{ updateDetails.body }}</p>
                    </div>
                  </div>
                </div>
              </section>
            </div>
          </main>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.settings-overlay {
  position: fixed;
  z-index: 5200;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 28px;
  background: rgb(0 0 0 / 0.44);
}

.settings-window {
  display: grid;
  grid-template-columns: 250px minmax(480px, 620px);
  width: min(880px, calc(100vw - 56px));
  height: min(640px, calc(100vh - 56px));
  min-height: 500px;
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 17px;
  background: var(--modal-bg);
  box-shadow: var(--shadow-overlay);
}

.settings-sidebar {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 16px;
  padding: 16px 12px;
  border-right: 1px solid var(--separator);
  background: color-mix(in srgb, var(--sidebar-bg) 84%, transparent);
}

.settings-search {
  display: flex;
  align-items: center;
  gap: 7px;
  height: 34px;
  padding: 0 10px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--icon);
}

.settings-search input {
  min-width: 0;
  width: 100%;
  border: 0;
  background: transparent;
  color: var(--text);
  font-size: 14px;
  font-weight: 560;
  outline: 0;
}

.settings-search input::placeholder {
  color: var(--text-muted);
}

.settings-nav {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 3px;
  overflow-y: auto;
}

.settings-nav-item {
  display: flex;
  min-width: 0;
  width: 100%;
  align-items: center;
  gap: 10px;
  height: 36px;
  padding: 0 10px;
  border-radius: 8px;
  background: transparent;
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 660;
  text-align: left;
}

.settings-nav-item:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.settings-nav-item.active {
  background: var(--btn-active-bg);
  color: var(--text);
  box-shadow: var(--btn-active-shadow);
}

.settings-nav-item span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.settings-main {
  display: flex;
  min-height: 0;
  min-width: 0;
  flex-direction: column;
  overflow: hidden;
  background: color-mix(in srgb, var(--pane-glass) 86%, transparent);
}

.settings-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 56px;
  flex: 0 0 auto;
  padding: 0 18px;
  border-bottom: 1px solid transparent;
}

.settings-main--content-scrollable .settings-header {
  border-bottom-color: var(--separator);
}

.settings-title-group {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 10px;
}

.settings-header-icon {
  display: grid;
  width: 30px;
  height: 30px;
  place-items: center;
  border-radius: 8px;
  background: var(--btn-active-bg);
  color: var(--text);
  box-shadow: var(--btn-active-shadow);
}

.settings-header h2 {
  overflow: hidden;
  margin: 0;
  color: var(--text);
  font-size: 18px;
  font-weight: 760;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.settings-close {
  display: grid;
  width: 26px;
  height: 26px;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.settings-close:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.settings-content {
  min-height: 0;
  flex: 1;
  overflow-y: auto;
  padding: 22px 26px 28px;
}

.settings-page {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.settings-section-heading {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 0 2px 4px;
}

.settings-section-heading h3 {
  margin: 0;
  color: var(--text);
  font-size: 15px;
  font-weight: 740;
  letter-spacing: 0;
}

.settings-section-heading p {
  max-width: 480px;
  margin: 0;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 560;
  line-height: 1.45;
}

.settings-group {
  overflow: hidden;
  border: 1px solid var(--hairline);
  border-radius: 12px;
  background: color-mix(in srgb, var(--text) 3.5%, transparent);
}

.settings-group--tools {
  overflow: visible;
}

.setting-row {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
  min-height: 62px;
  padding: 14px 14px;
}

.setting-row + .setting-row {
  border-top: 1px solid var(--hairline);
}

.setting-row--stacked {
  align-items: stretch;
  flex-direction: column;
  gap: 12px;
}

.setting-row--switch {
  cursor: pointer;
}

.setting-copy {
  display: flex;
  min-width: 0;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 3px;
}

.setting-copy strong {
  overflow-wrap: anywhere;
  color: var(--text);
  font-size: 13px;
  font-weight: 720;
  letter-spacing: 0;
}

.setting-copy span {
  max-width: 430px;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 540;
  line-height: 1.4;
}

.appearance-options {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 10px;
}

.appearance-option {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 8px;
  align-items: stretch;
  padding: 8px;
  border: 1px solid var(--hairline);
  border-radius: 10px;
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
  color: var(--text-muted);
}

.appearance-option:hover {
  border-color: var(--control-border);
  background: var(--btn-hover);
  color: var(--text);
}

.appearance-option.active {
  border-color: var(--accent-border);
  background: color-mix(in srgb, var(--accent) 13%, transparent);
  box-shadow: inset 0 0 0 1px rgb(var(--accent-rgb) / 0.24);
  color: var(--text);
}

.appearance-preview {
  position: relative;
  display: grid;
  grid-template-columns: 32% 1fr;
  grid-template-rows: 18px 1fr;
  height: 78px;
  overflow: hidden;
  border-radius: 8px;
  background: #202521;
  box-shadow: inset 0 0 0 1px rgb(255 255 255 / 0.08);
}

.preview-sidebar {
  grid-row: 1 / 3;
  background: #171b18;
}

.preview-toolbar {
  background: #303531;
}

.preview-pane {
  background: #242a26;
}

.preview-accent {
  position: absolute;
  right: 11px;
  bottom: 10px;
  width: 38px;
  height: 6px;
  border-radius: 6px;
  background: var(--accent);
}

.appearance-option--light .appearance-preview {
  background: #f3f5f0;
  box-shadow: inset 0 0 0 1px rgb(28 36 24 / 0.10);
}

.appearance-option--light .preview-sidebar { background: #dde1da; }
.appearance-option--light .preview-toolbar { background: #eceeea; }
.appearance-option--light .preview-pane { background: #f8faf5; }

.appearance-option--system .appearance-preview {
  background:
    linear-gradient(90deg, #f3f5f0 0 50%, #202521 50% 100%);
}

.appearance-option--system .preview-sidebar {
  background:
    linear-gradient(90deg, #dde1da 0 50%, #171b18 50% 100%);
}

.appearance-option--system .preview-toolbar {
  background:
    linear-gradient(90deg, #eceeea 0 50%, #303531 50% 100%);
}

.appearance-option--system .preview-pane {
  background:
    linear-gradient(90deg, #f8faf5 0 50%, #242a26 50% 100%);
}

.appearance-label {
  display: inline-flex;
  min-width: 0;
  align-items: center;
  justify-content: center;
  gap: 6px;
  color: inherit;
  font-size: 12px;
  font-weight: 680;
}

.scheme-options {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
}

.scheme-option {
  display: grid;
  min-width: 0;
  grid-template-columns: 88px minmax(0, 1fr) auto;
  align-items: center;
  gap: 10px;
  padding: 8px;
  border: 1px solid var(--hairline);
  border-radius: 10px;
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
  color: var(--text-muted);
  text-align: left;
}

.scheme-option:hover {
  border-color: var(--control-border);
  background: var(--btn-hover);
  color: var(--text);
}

.scheme-option.active {
  border-color: var(--accent-border);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  box-shadow: inset 0 0 0 1px rgb(var(--accent-rgb) / 0.22);
  color: var(--text);
}

.scheme-preview {
  position: relative;
  display: grid;
  grid-template-columns: 34% 1fr;
  grid-template-rows: 16px 1fr;
  height: 54px;
  overflow: hidden;
  border-radius: 8px;
  background: var(--scheme-pane);
  box-shadow: inset 0 0 0 1px rgb(255 255 255 / 0.10);
}

.scheme-preview-sidebar {
  grid-row: 1 / 3;
  background: var(--scheme-sidebar);
}

.scheme-preview-toolbar {
  background: var(--scheme-toolbar);
}

.scheme-preview-pane {
  background: var(--scheme-pane);
}

.scheme-preview-accent {
  position: absolute;
  right: 8px;
  bottom: 8px;
  width: 28px;
  height: 5px;
  border-radius: 5px;
  background: var(--scheme-accent);
}

.scheme-details {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 2px;
}

.scheme-name {
  overflow: hidden;
  color: inherit;
  font-size: 12px;
  font-weight: 720;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.scheme-description {
  display: -webkit-box;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 540;
  line-height: 1.3;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.scheme-swatches {
  display: inline-flex;
  align-self: start;
  gap: 4px;
  padding-top: 2px;
}

.scheme-swatches span {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  box-shadow: inset 0 0 0 1px rgb(255 255 255 / 0.18);
}

.accent-color-control {
  display: grid;
  min-width: 0;
  gap: 10px;
}

.accent-color-grid {
  display: grid;
  min-width: 0;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 8px;
}

.accent-color-option,
.accent-color-custom {
  display: grid;
  min-width: 0;
  min-height: 48px;
  grid-template-columns: 26px minmax(0, 1fr);
  align-items: center;
  gap: 10px;
  padding: 9px 10px;
  border: 1px solid var(--hairline);
  border-radius: 10px;
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
  color: var(--text-muted);
  text-align: left;
}

.accent-color-option {
  cursor: pointer;
}

.accent-color-option--default {
  grid-column: span 2;
}

.accent-color-option:hover,
.accent-color-custom:hover {
  border-color: var(--control-border);
  background: var(--btn-hover);
  color: var(--text);
}

.accent-color-option:focus-visible,
.accent-color-custom:focus-within {
  border-color: var(--accent-border);
  box-shadow: var(--accent-focus-ring);
  outline: 0;
}

.accent-color-option.active {
  border-color: var(--accent-border);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  box-shadow: inset 0 0 0 1px rgb(var(--accent-rgb) / 0.22);
  color: var(--text);
}

.accent-color-swatch {
  width: 22px;
  height: 22px;
  border-radius: 7px;
  background:
    linear-gradient(135deg, rgb(255 255 255 / 0.24), transparent 42%),
    var(--accent-choice);
  box-shadow:
    inset 0 0 0 1px rgb(255 255 255 / 0.24),
    0 3px 10px color-mix(in srgb, var(--accent-choice) 26%, transparent);
}

.accent-color-option-copy,
.accent-color-custom-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.accent-color-option-copy strong,
.accent-color-option-copy span,
.accent-color-custom-copy strong,
.accent-color-custom-copy span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.accent-color-option-copy strong,
.accent-color-custom-copy strong {
  color: inherit;
  font-size: 12px;
  font-weight: 720;
}

.accent-color-option-copy span,
.accent-color-custom-copy span {
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 560;
  font-variant-numeric: tabular-nums;
}

.accent-color-custom {
  grid-template-columns: minmax(0, 1fr) auto;
  border-color: var(--input-border);
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  cursor: pointer;
}

.accent-color-custom-picker {
  position: relative;
  display: inline-grid;
  height: 32px;
  grid-template-columns: 22px auto;
  align-items: center;
  justify-content: center;
  gap: 7px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 0 11px;
  background: color-mix(in srgb, var(--text) 4%, transparent);
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 660;
  white-space: nowrap;
}

.accent-color-custom-picker input {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  opacity: 0;
  cursor: pointer;
}

.accent-color-custom-picker:hover {
  border-color: var(--control-border);
  color: var(--text);
}

.accent-color-custom.active {
  border-color: var(--accent-border);
  background: color-mix(in srgb, var(--accent) 10%, transparent);
  box-shadow:
    inset 0 0 0 1px rgb(var(--accent-rgb) / 0.20),
    var(--input-shadow);
  color: var(--text);
}

.view-segment {
  display: inline-flex;
  width: fit-content;
  max-width: 100%;
  overflow: hidden;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
}

.view-segment button {
  display: inline-flex;
  min-width: 96px;
  height: 34px;
  align-items: center;
  justify-content: center;
  gap: 7px;
  padding: 0 11px;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 660;
}

.view-segment button span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.delete-mode-segment button {
  min-width: 152px;
}

.view-segment button + button {
  border-left: 1px solid var(--hairline);
}

.view-segment button:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.view-segment button.active {
  background: var(--btn-active-bg);
  color: var(--text);
}

.date-format-options {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(172px, 1fr));
  gap: 8px;
}

.date-format-option {
  display: grid;
  min-width: 0;
  min-height: 58px;
  grid-template-columns: minmax(0, 1fr) 18px;
  align-items: center;
  gap: 10px;
  padding: 9px 10px;
  border: 1px solid var(--hairline);
  border-radius: 10px;
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
  color: var(--text-muted);
  text-align: left;
}

.date-format-option:hover {
  border-color: var(--control-border);
  background: var(--btn-hover);
  color: var(--text);
}

.date-format-option:focus-visible {
  border-color: var(--accent-border);
  box-shadow: var(--accent-focus-ring);
  outline: 0;
}

.date-format-option.active {
  border-color: var(--accent-border);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  box-shadow: inset 0 0 0 1px rgb(var(--accent-rgb) / 0.22);
  color: var(--text);
}

.date-format-copy {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.date-format-copy strong,
.date-format-copy span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.date-format-copy strong {
  color: inherit;
  font-size: 12px;
  font-weight: 720;
}

.date-format-copy span {
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 560;
  font-variant-numeric: tabular-nums;
}

.date-format-check {
  display: grid;
  width: 18px;
  height: 18px;
  place-items: center;
  border-radius: 50%;
  color: var(--accent);
  opacity: 0;
  transform: scale(0.82);
  transition: opacity 100ms ease, transform 100ms ease;
}

.date-format-check.visible {
  opacity: 1;
  transform: scale(1);
}

.custom-tool-list {
  display: grid;
  gap: 10px;
}

.editor-command-field,
.custom-tool-card {
  display: grid;
  gap: 10px;
  min-width: 0;
  padding: 10px;
  border: 1px solid var(--hairline);
  border-radius: 10px;
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
}

.editor-command-field {
  padding: 10px 12px;
}

.custom-tool-card--disabled {
  opacity: 0.62;
}

.custom-tool-header {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto auto;
  align-items: end;
  gap: 10px;
}

.editor-command-input,
.custom-tool-field {
  display: grid;
  min-width: 0;
  gap: 5px;
}

.editor-command-input span,
.custom-tool-field span {
  color: var(--text-muted);
  font-size: 10px;
  font-weight: 720;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.editor-command-input input,
.custom-tool-field input {
  width: 100%;
  min-width: 0;
  height: 34px;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  padding: 0 10px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text);
  font: inherit;
  font-size: 12.5px;
  font-weight: 590;
  outline: 0;
}

.editor-template-control {
  position: relative;
  width: fit-content;
  max-width: 100%;
}

.editor-template-link {
  width: fit-content;
  max-width: 100%;
  padding: 0;
  background: transparent;
  color: var(--text-faint);
  cursor: pointer;
  font-size: 12px;
  font-weight: 590;
  text-align: left;
  text-decoration: underline;
  text-decoration-color: transparent;
  text-underline-offset: 3px;
  transition: color 100ms ease, text-decoration-color 100ms ease;
}

.editor-template-link:hover,
.editor-template-link:focus-visible,
.editor-template-link--open {
  color: var(--text-muted);
  text-decoration-color: currentColor;
}

.editor-template-popover {
  position: absolute;
  top: calc(100% + 7px);
  left: 0;
  z-index: 5300;
  width: min(310px, calc(100vw - 72px));
  max-height: min(340px, 48vh);
  overflow-y: auto;
  overscroll-behavior: contain;
  border: 1px solid var(--control-border);
  border-radius: 13px;
  padding: 5px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  transform-origin: top left;
  scrollbar-width: thin;
  scrollbar-color: var(--control-border) transparent;
}

.editor-template-popover::-webkit-scrollbar {
  width: 9px;
}

.editor-template-popover::-webkit-scrollbar-track {
  background: transparent;
}

.editor-template-popover::-webkit-scrollbar-thumb {
  border: 2px solid transparent;
  border-radius: 999px;
  background: var(--control-border);
  background-clip: padding-box;
}

.editor-template-option {
  display: grid;
  width: 100%;
  min-height: 38px;
  grid-template-columns: minmax(0, 1fr) 16px;
  align-items: center;
  gap: 9px;
  padding: 5px 9px;
  border-radius: 8px;
  background: transparent;
  color: var(--text);
  cursor: pointer;
  text-align: left;
  transition: none;
}

.editor-template-option span {
  display: grid;
  min-width: 0;
  gap: 1px;
}

.editor-template-option strong,
.editor-template-option small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.editor-template-option strong {
  font-size: 13px;
  font-weight: 540;
}

.editor-template-option small {
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 520;
}

.editor-template-option:hover,
.editor-template-option:focus-visible {
  background: var(--btn-primary-bg);
  color: #fff;
  outline: 0;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.editor-template-option:hover small,
.editor-template-option:focus-visible small {
  color: rgb(255 255 255 / 0.78);
}

.editor-template-option--active {
  background: var(--btn-active-bg);
  box-shadow: var(--btn-active-shadow);
}

.editor-template-popover-enter-active,
.editor-template-popover-leave-active {
  transition: opacity 130ms cubic-bezier(0.2, 0, 0, 1), transform 130ms cubic-bezier(0.2, 0, 0, 1);
}

.editor-template-popover-enter-from,
.editor-template-popover-leave-to {
  opacity: 0;
  transform: scale(0.96);
}

.editor-command-input input::placeholder,
.custom-tool-field input::placeholder {
  color: var(--text-faint);
}

.editor-command-input input:focus-visible,
.custom-tool-field input:focus-visible {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.custom-tool-scope {
  display: grid;
  min-width: 0;
  gap: 6px;
}

.custom-tool-scope > span {
  color: var(--text-muted);
  font-size: 10px;
  font-weight: 720;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.custom-tool-targets {
  display: inline-flex;
  width: fit-content;
  max-width: 100%;
  overflow: hidden;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
}

.custom-tool-targets button {
  min-width: 74px;
  height: 30px;
  padding: 0 10px;
  background: transparent;
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 680;
}

.custom-tool-targets button + button {
  border-left: 1px solid var(--hairline);
}

.custom-tool-targets button:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.custom-tool-targets button.active {
  background: var(--btn-active-bg);
  color: var(--text);
}

.custom-tool-toggle {
  display: inline-flex;
  align-items: center;
  height: 34px;
}

.custom-tool-remove,
.custom-tool-add {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--input-border);
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text-muted);
}

.custom-tool-remove {
  width: 34px;
  height: 34px;
  border-radius: 8px;
}

.custom-tool-remove:hover {
  border-color: rgb(var(--danger-rgb) / 0.32);
  background: rgb(var(--danger-rgb) / 0.10);
  color: var(--danger);
}

.custom-tool-empty {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 48px;
  border: 1px dashed var(--control-border);
  border-radius: 10px;
  padding: 0 12px;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 620;
}

.custom-tool-add {
  width: fit-content;
  min-height: 34px;
  gap: 7px;
  border-radius: 8px;
  padding: 0 12px;
  font-size: 12px;
  font-weight: 680;
}

.custom-tool-add:hover {
  border-color: var(--accent-border);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--text);
}

.about-group {
  display: grid;
}

.about-summary {
  display: grid;
  grid-template-columns: 96px minmax(0, 1fr);
  align-items: center;
  gap: 18px;
  min-height: 136px;
  padding: 16px;
}

.about-app-mark {
  display: grid;
  width: 96px;
  height: 96px;
  place-items: center;
  border-radius: 12px;
  background: transparent;
}

.about-app-mark img {
  display: block;
  width: 96px;
  height: 96px;
  object-fit: contain;
}

.about-copy {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.about-copy strong {
  color: var(--text);
  font-size: 20px;
  font-weight: 780;
  letter-spacing: 0;
}

.about-copy span {
  max-width: 430px;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 560;
  line-height: 1.45;
}

.about-details {
  display: grid;
  margin: 0;
  border-top: 1px solid var(--hairline);
}

.about-details div {
  display: grid;
  grid-template-columns: 126px minmax(0, 1fr);
  gap: 16px;
  min-height: 44px;
  align-items: center;
  padding: 9px 14px;
}

.about-details div + div {
  border-top: 1px solid var(--hairline);
}

.about-details dt {
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 720;
  letter-spacing: 0.04em;
  text-overflow: ellipsis;
  text-transform: uppercase;
  white-space: nowrap;
}

.about-details dd {
  min-width: 0;
  margin: 0;
  overflow-wrap: anywhere;
  color: var(--text);
  font-size: 12.5px;
  font-weight: 610;
  line-height: 1.35;
}

.about-update {
  display: grid;
  gap: 10px;
  border-top: 1px solid var(--hairline);
  padding: 14px;
}

.about-update-main {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
}

.about-update-copy {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.about-update-copy strong {
  color: var(--text);
  font-size: 12.5px;
  font-weight: 730;
}

.about-update-copy span {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 560;
  line-height: 1.35;
}

.about-update-copy .about-update-error {
  color: var(--danger);
}

.about-update-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-height: 34px;
  gap: 7px;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text-muted);
  padding: 0 12px;
  font-size: 12px;
  font-weight: 680;
  white-space: nowrap;
}

.about-update-button:hover:not(:disabled) {
  border-color: var(--accent-border);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--text);
}

.about-update-button:disabled {
  cursor: default;
  opacity: 0.65;
}

.about-update-progress {
  height: 5px;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text) 10%, transparent);
}

.about-update-progress span {
  display: block;
  height: 100%;
  border-radius: inherit;
  background: var(--accent);
  transition: width 140ms ease;
}

.about-update-release {
  display: grid;
  gap: 6px;
  border: 1px solid var(--hairline);
  border-radius: 10px;
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
  padding: 10px 12px;
}

.about-update-release span {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
}

.about-update-release strong {
  color: var(--text);
  font-weight: 730;
}

.about-update-release p {
  max-height: 92px;
  margin: 0;
  overflow: auto;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 520;
  line-height: 1.45;
  white-space: pre-wrap;
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

.settings-fade-enter-active,
.settings-fade-leave-active {
  transition: opacity 130ms ease;
}

.settings-fade-enter-active .settings-window,
.settings-fade-leave-active .settings-window {
  transition: transform 130ms ease, opacity 130ms ease;
}

.settings-fade-enter-from,
.settings-fade-leave-to {
  opacity: 0;
}

.settings-fade-enter-from .settings-window,
.settings-fade-leave-to .settings-window {
  opacity: 0;
  transform: translateY(8px) scale(0.985);
}

@media (max-width: 760px) {
  .settings-overlay {
    padding: 14px;
  }

  .settings-window {
    grid-template-columns: 1fr;
    width: calc(100vw - 28px);
    height: calc(100vh - 28px);
  }

  .settings-sidebar {
    display: none;
  }

  .appearance-options {
    grid-template-columns: 1fr;
  }

  .scheme-options {
    grid-template-columns: 1fr;
  }

  .scheme-option {
    grid-template-columns: 80px minmax(0, 1fr) auto;
  }

  .accent-color-control {
    gap: 9px;
  }

  .accent-color-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .accent-color-option--default {
    grid-column: 1 / -1;
  }

  .accent-color-custom {
    grid-template-columns: 1fr;
    align-items: stretch;
  }

  .accent-color-custom-picker {
    width: 100%;
  }

  .view-segment {
    width: 100%;
  }

  .view-segment button {
    min-width: 0;
    flex: 1;
  }

  .custom-tool-header {
    grid-template-columns: minmax(0, 1fr) auto;
  }

  .custom-tool-remove {
    grid-column: 2;
  }

  .custom-tool-targets {
    width: 100%;
  }

  .custom-tool-targets button {
    min-width: 0;
    flex: 1;
  }

  .about-details div {
    grid-template-columns: 1fr;
    gap: 3px;
    align-items: start;
  }

  .about-update-main {
    align-items: stretch;
    flex-direction: column;
  }

  .about-update-button {
    width: 100%;
  }
}
</style>
