<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { useFileManagerStore } from '../stores/fileManagerStore';

const store = useFileManagerStore();
const searchQuery = ref('');
const activeSectionId = ref('appearance');

const sections = [
  {
    id: 'appearance',
    label: 'Appearance',
    icon: 'monitor',
    keywords: 'appearance theme color dark light auto system',
  },
  {
    id: 'files',
    label: 'Files',
    icon: 'folder',
    keywords: 'files hidden default view list grid columns',
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
    keywords: 'safety confirm delete remove destructive',
  },
  {
    id: 'terminal',
    label: 'Terminal',
    icon: 'terminal',
    keywords: 'terminal shell cwd active folder directory',
  },
];

const appearanceModes = [
  { value: 'system', label: 'Auto', icon: 'monitor' },
  { value: 'light', label: 'Light', icon: 'sun' },
  { value: 'dark', label: 'Dark', icon: 'moon' },
];

const viewModes = [
  { value: 'list', label: 'List', icon: 'list' },
  { value: 'grid', label: 'Grid', icon: 'grid' },
  { value: 'columns', label: 'Columns', icon: 'columns' },
];

const activeSection = computed(() =>
  sections.find((section) => section.id === activeSectionId.value) || sections[0],
);

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

function setDefaultViewMode(viewMode) {
  store.setAppSetting('defaultViewMode', viewMode);
}

function setBooleanSetting(key, event) {
  store.setAppSetting(key, event.target.checked);
}

function close() {
  store.closeSettings();
}

function onKeydown(event) {
  if (event.key === 'Escape' && store.settingsVisible) {
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
    }
  },
);

onMounted(() => window.addEventListener('keydown', onKeydown));
onUnmounted(() => window.removeEventListener('keydown', onKeydown));
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
            <div class="settings-traffic" aria-hidden="true">
              <span class="traffic-dot traffic-dot--close"></span>
              <span class="traffic-dot traffic-dot--minimize"></span>
              <span class="traffic-dot traffic-dot--zoom"></span>
            </div>

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

          <main class="settings-main">
            <header class="settings-header">
              <div class="settings-title-group">
                <div class="settings-header-icon">
                  <AppIcon :name="activeSection.icon" :size="19" :stroke-width="1.8" />
                </div>
                <h2 id="settings-title">{{ activeSection.label }}</h2>
              </div>

              <button type="button" class="settings-close" aria-label="Close settings" @click="close">
                <AppIcon name="x" :size="16" :stroke-width="2" />
              </button>
            </header>

            <div class="settings-content">
              <section v-if="activeSectionId === 'appearance'" class="settings-page">
                <div class="settings-section-heading">
                  <h3>Appearance</h3>
                  <p>Choose how Carelo follows or overrides the system color scheme.</p>
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
  backdrop-filter: blur(18px) saturate(1.05);
  -webkit-backdrop-filter: blur(18px) saturate(1.05);
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

.settings-traffic {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 18px;
  padding-left: 2px;
}

.traffic-dot {
  display: block;
  width: 12px;
  height: 12px;
  border-radius: 50%;
  box-shadow: inset 0 0 0 0.5px rgb(0 0 0 / 0.28);
}

.traffic-dot--close { background: var(--traffic-close); }
.traffic-dot--minimize { background: var(--traffic-minimize); }
.traffic-dot--zoom { background: var(--traffic-zoom); }

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
  min-width: 0;
  flex-direction: column;
  background: color-mix(in srgb, var(--pane-glass) 86%, transparent);
}

.settings-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 56px;
  flex: 0 0 auto;
  padding: 0 18px;
  border-bottom: 1px solid var(--separator);
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
  width: 31px;
  height: 31px;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
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

  .view-segment {
    width: 100%;
  }

  .view-segment button {
    min-width: 0;
    flex: 1;
  }
}
</style>
