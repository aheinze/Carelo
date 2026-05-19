<script setup>
import { computed, defineAsyncComponent, onBeforeUnmount, ref, watch } from 'vue';
import WindowResizeHandles from './components/WindowResizeHandles.vue';
import Toolbar from './components/Toolbar.vue';
import Sidebar from './components/Sidebar.vue';
import Pane from './components/Pane.vue';
import PreviewPanel from './components/PreviewPanel.vue';
import TransferQueue from './components/TransferQueue.vue';
import TooltipHost from './components/TooltipHost.vue';
import { useDialog } from './composables/useDialog';
import { useKeyboardShortcuts } from './composables/useKeyboardShortcuts';
import { useShortcutsModal } from './composables/useShortcutsModal';
import { useFileManagerStore } from './stores/fileManagerStore';

const CommandPalette = defineAsyncComponent(() => import('./components/CommandPalette.vue'));
const DialogHost = defineAsyncComponent(() => import('./components/DialogHost.vue'));
const SettingsWindow = defineAsyncComponent(() => import('./components/SettingsWindow.vue'));
const ShortcutsModal = defineAsyncComponent(() => import('./components/ShortcutsModal.vue'));
const TerminalPanel = defineAsyncComponent(() => import('./components/TerminalPanel.vue'));

const store = useFileManagerStore();
const dialog = useDialog();
const shortcutsModal = useShortcutsModal();
const appWindow = ref(null);
const workspace = ref(null);
const paneGrid = ref(null);
const terminalPanelMounted = ref(store.terminalPanelVisible);
let stopResize = null;

useKeyboardShortcuts();

const layoutStyle = computed(() => ({
  '--sidebar-width': `${store.sidebarWidth}px`,
  '--preview-panel-width': `${store.previewPanelWidth}px`,
  '--left-pane-width': `${store.paneSplitPercent}%`,
  '--terminal-panel-height': `${store.terminalPanelHeight}px`,
}));
const dialogVisible = computed(() => Boolean(dialog.activeDialog.value));
const shortcutsVisible = computed(() => shortcutsModal.visible.value);

watch(
  () => store.terminalPanelVisible,
  (visible) => {
    if (visible) {
      terminalPanelMounted.value = true;
    }
  },
  { immediate: true },
);

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function startResize(event, onMove) {
  event.preventDefault();
  event.stopPropagation();

  stopResize?.();
  document.body.classList.add('is-panel-resizing');

  const handlePointerMove = (moveEvent) => {
    moveEvent.preventDefault();
    onMove(moveEvent);
  };

  const handlePointerUp = () => {
    document.body.classList.remove('is-panel-resizing');
    window.removeEventListener('pointermove', handlePointerMove);
    window.removeEventListener('pointerup', handlePointerUp);
    stopResize = null;
  };

  window.addEventListener('pointermove', handlePointerMove);
  window.addEventListener('pointerup', handlePointerUp);
  stopResize = handlePointerUp;
}

function startSidebarResize(event) {
  const rect = appWindow.value?.getBoundingClientRect();

  if (!rect) {
    return;
  }

  startResize(event, (moveEvent) => {
    const availableWidth = Math.max(210, rect.width - 720);
    store.setSidebarWidth(Math.min(availableWidth, moveEvent.clientX - rect.left));
  });
}

function startPaneResize(event) {
  const rect = paneGrid.value?.getBoundingClientRect();

  if (!rect) {
    return;
  }

  startResize(event, (moveEvent) => {
    const minPercent = (220 / rect.width) * 100;
    const maxPercent = ((rect.width - 225) / rect.width) * 100;
    const percent = clamp(((moveEvent.clientX - rect.left) / rect.width) * 100, minPercent, maxPercent);
    store.setPaneSplitPercent(percent);
  });
}

function startPreviewResize(event) {
  const rect = workspace.value?.getBoundingClientRect();

  if (!rect) {
    return;
  }

  startResize(event, (moveEvent) => {
    const availableWidth = Math.max(280, rect.width - 460);
    store.setPreviewPanelWidth(Math.min(availableWidth, rect.right - moveEvent.clientX));
  });
}

onBeforeUnmount(() => {
  stopResize?.();
});
</script>

<template>
  <div
    ref="appWindow"
    class="app-window"
    :class="{ 'app-window--sidebar-hidden': !store.sidebarVisible }"
    :style="layoutStyle"
  >
    <Sidebar />
    <div
      class="panel-resize-handle panel-resize-handle--sidebar"
      aria-hidden="true"
      @dblclick="store.setSidebarWidth(310)"
      @pointerdown="startSidebarResize"
    ></div>

    <div
      class="main-shell"
      :class="{ 'main-shell--terminal-visible': store.terminalPanelVisible }"
    >
      <Toolbar />

      <main
        ref="workspace"
        class="workspace"
        :class="{ 'workspace--preview-hidden': !store.previewPanelVisible }"
      >
        <section ref="paneGrid" class="pane-grid" aria-label="File panes">
          <Pane pane-id="left" title="Left" />
          <div
            class="panel-resize-handle panel-resize-handle--panes"
            aria-hidden="true"
            @dblclick="store.setPaneSplitPercent(48)"
            @pointerdown="startPaneResize"
          ></div>
          <Pane pane-id="right" title="Right" />
        </section>

        <div
          class="panel-resize-handle panel-resize-handle--preview"
          aria-hidden="true"
          @dblclick="store.setPreviewPanelWidth(400)"
          @pointerdown="startPreviewResize"
        ></div>
        <PreviewPanel />
      </main>

      <TerminalPanel
        v-if="terminalPanelMounted"
        :visible="store.terminalPanelVisible"
      />

      <TransferQueue v-if="store.queue.length > 0" />
    </div>

    <CommandPalette />
    <SettingsWindow v-if="store.settingsVisible" />
    <DialogHost v-if="dialogVisible" />
    <ShortcutsModal v-if="shortcutsVisible" />
    <TooltipHost />
    <WindowResizeHandles />
  </div>
</template>

<style>
/* App shell styling is centralized in src/assets/main.css. */
</style>
