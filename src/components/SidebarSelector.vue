<script setup>
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { useDialog } from '../composables/useDialog';
import { useFileManagerStore } from '../stores/fileManagerStore';

const RemoteVolumeModal = defineAsyncComponent(() => import('./RemoteVolumeModal.vue'));

const store = useFileManagerStore();
const dialog = useDialog();
const selectorRef = ref(null);
const open = ref(false);
const remoteModalVisible = ref(false);
const mountingDevicePath = ref('');

const activePath = computed(() => store.effectiveDirectoryFor(store.activePaneId) || '');
const activeItem = computed(() => {
  let bestMatch = null;
  let bestLength = -1;

  for (const section of store.sidebarSections) {
    for (const item of section.items || []) {
      if (!item.path || !sidebarPathMatches(item, activePath.value)) {
        continue;
      }

      if (item.path.length > bestLength) {
        bestMatch = item;
        bestLength = item.path.length;
      }
    }
  }

  return bestMatch;
});

function sidebarPathMatches(item, path) {
  if (!item?.path || !path) {
    return false;
  }

  const itemPath = normalizeComparablePath(item.path);
  const targetPath = normalizeComparablePath(path);

  if (targetPath === itemPath) {
    return true;
  }

  if (!item.matchPrefix) {
    return false;
  }

  if (itemPath === '/') {
    return targetPath.startsWith('/');
  }

  return targetPath.startsWith(`${itemPath}/`);
}

function normalizeComparablePath(path) {
  const value = String(path || '').trim();

  if (!value || value === '/') {
    return value || '';
  }

  return value.replace(/\/+$/, '');
}

function togglePopover(event) {
  event?.stopPropagation();
  open.value = !open.value;
}

function closePopover() {
  open.value = false;
}

async function selectSidebarItem(item) {
  if (item.disabled || (item.devicePath && mountingDevicePath.value === item.devicePath)) {
    return;
  }

  if (item.isMountable && item.devicePath) {
    await mountSidebarVolume(item);
    return;
  }

  if (item.path) {
    store.setPanePath(store.activePaneId, item.path);
    closePopover();
  }
}

async function mountSidebarVolume(item) {
  mountingDevicePath.value = item.devicePath;

  try {
    const volume = await store.mountLocalVolume(item);
    await store.refreshVolumes();

    if (volume?.path) {
      store.setPanePath(store.activePaneId, volume.path);
      closePopover();
    }
  } catch (error) {
    await store.refreshVolumes();
    await dialog.alert({
      title: item.needsUnlock ? 'Unlock Failed' : 'Mount Failed',
      message: error?.message || `Unable to ${item.needsUnlock ? 'unlock' : 'mount'} ${item.name}.`,
      detail: item.devicePath || '',
      variant: 'warning',
    });
  } finally {
    mountingDevicePath.value = '';
  }
}

function deviceBusyLabel(item) {
  return item?.needsUnlock ? 'Unlocking...' : 'Mounting...';
}

function openRemoteModal() {
  closePopover();
  remoteModalVisible.value = true;
}

function closeRemoteModal() {
  remoteModalVisible.value = false;
}

async function createFavoriteGroup() {
  closePopover();

  const name = (await dialog.prompt({
    title: 'New Sidebar Group',
    message: 'Create a section for sidebar shortcuts.',
    inputLabel: 'Group name',
    inputValue: 'New Group',
    inputRequired: true,
    confirmLabel: 'Create',
    icon: 'folder-plus',
  }))?.trim();

  if (!name) {
    return;
  }

  try {
    await store.addFavoriteGroup(name);
  } catch (error) {
    await dialog.alert({
      title: 'Group Not Created',
      message: error?.message || 'Unable to create sidebar group.',
      variant: 'warning',
    });
  }
}

function handleDocumentPointerDown(event) {
  if (!open.value || selectorRef.value?.contains(event.target)) {
    return;
  }

  closePopover();
}

function handleKeydown(event) {
  if (event.key === 'Escape' && open.value) {
    closePopover();
  }
}

watch(
  () => store.sidebarVisible,
  (visible) => {
    if (visible) {
      closePopover();
    }
  },
);

onMounted(() => {
  document.addEventListener('pointerdown', handleDocumentPointerDown, true);
  window.addEventListener('keydown', handleKeydown);
});

onUnmounted(() => {
  document.removeEventListener('pointerdown', handleDocumentPointerDown, true);
  window.removeEventListener('keydown', handleKeydown);
});
</script>

<template>
  <div
    ref="selectorRef"
    class="sidebar-selector"
    @mousedown.stop
    @dblclick.stop
  >
    <button
      v-tooltip="{ text: 'Sidebar locations', description: 'Open locations, devices, favorites, and remote storage' }"
      type="button"
      class="sidebar-selector-trigger"
      :class="{ 'sidebar-selector-trigger--open': open }"
      aria-haspopup="menu"
      :aria-expanded="open"
      aria-label="Open sidebar locations"
      @click="togglePopover"
    >
      <AppIcon name="menu" :size="18" :stroke-width="1.9" />
    </button>

    <Transition name="sidebar-selector-popover">
      <div
        v-if="open"
        class="sidebar-selector-popover"
        role="menu"
        aria-label="Sidebar locations"
        @click.stop
      >
        <div class="sidebar-selector-scroll">
          <section
            v-for="section in store.sidebarSections"
            :key="section.id || section.title"
            class="sidebar-selector-section"
          >
            <h2>{{ section.title }}</h2>
            <button
              v-for="item in section.items || []"
              :key="`${section.title}-${item.id || item.path || item.devicePath || item.name}`"
              type="button"
              role="menuitem"
              class="sidebar-selector-item"
              :class="{
                'sidebar-selector-item--active': activeItem && item.path === activeItem.path,
                'sidebar-selector-item--disabled': item.disabled,
                'sidebar-selector-item--remote': item.isRemote,
              }"
              :disabled="item.disabled || mountingDevicePath === item.devicePath"
              @click="selectSidebarItem(item)"
            >
              <span class="sidebar-selector-item-icon" :style="{ '--item-color': item.color }" aria-hidden="true">
                <AppIcon :name="item.icon || 'folder'" :size="17" :stroke-width="1.9" />
              </span>
              <span class="sidebar-selector-item-copy">
                <span>{{ item.name }}</span>
                <small v-if="item.detail || mountingDevicePath === item.devicePath">
                  {{ mountingDevicePath === item.devicePath ? deviceBusyLabel(item) : item.detail }}
                </small>
              </span>
            </button>
            <p v-if="(section.items || []).length === 0" class="sidebar-selector-empty">No items</p>
          </section>
        </div>

        <div class="sidebar-selector-actions">
          <button type="button" role="menuitem" @click="store.toggleSidebar">
            <AppIcon name="sidebar" :size="16" :stroke-width="1.9" />
            <span>Show Sidebar</span>
          </button>
          <button type="button" role="menuitem" @click="openRemoteModal">
            <AppIcon name="network" :size="16" :stroke-width="1.9" />
            <span>Remote Storage</span>
          </button>
          <button type="button" role="menuitem" @click="createFavoriteGroup">
            <AppIcon name="folder-plus" :size="16" :stroke-width="1.9" />
            <span>New Group</span>
          </button>
        </div>
      </div>
    </Transition>

    <RemoteVolumeModal
      v-if="remoteModalVisible"
      :visible="remoteModalVisible"
      @close="closeRemoteModal"
    />
  </div>
</template>

<style scoped>
.sidebar-selector {
  position: relative;
  flex: 0 0 auto;
}

.sidebar-selector-trigger {
  display: inline-flex;
  width: 31px;
  height: 34px;
  align-items: center;
  justify-content: center;
  padding: 0;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition: background 80ms ease, color 80ms ease;
}

.sidebar-selector-trigger:hover,
.sidebar-selector-trigger--open {
  background: var(--btn-hover);
  color: var(--text);
}

.sidebar-selector-item-icon {
  display: inline-flex;
  flex: 0 0 auto;
  color: var(--item-color, var(--icon));
}

.sidebar-selector-popover {
  position: absolute;
  top: calc(100% + 14px);
  left: 0;
  z-index: 2000;
  width: min(332px, calc(100vw - 28px));
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  padding: 5px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  transform-origin: top left;
}

.sidebar-selector-scroll {
  max-height: min(58vh, 460px);
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 0;
  scrollbar-width: thin;
  scrollbar-color: var(--control-border) transparent;
}

.sidebar-selector-scroll::-webkit-scrollbar {
  width: 9px;
}

.sidebar-selector-scroll::-webkit-scrollbar-track {
  background: transparent;
}

.sidebar-selector-scroll::-webkit-scrollbar-thumb {
  border: 2px solid transparent;
  border-radius: 999px;
  background: var(--control-border);
  background-clip: padding-box;
}

.sidebar-selector-section + .sidebar-selector-section {
  margin-top: 4px;
  padding-top: 4px;
  border-top: 1px solid var(--hairline);
}

.sidebar-selector-section h2 {
  margin: 0;
  padding: 5px 10px 6px;
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 620;
  letter-spacing: 0.05em;
  text-transform: uppercase;
}

.sidebar-selector-item {
  display: grid;
  width: 100%;
  min-height: 28px;
  grid-template-columns: 18px minmax(0, 1fr);
  align-items: center;
  gap: 9px;
  padding: 0 10px;
  border-radius: 8px;
  background: transparent;
  color: var(--text);
  cursor: pointer;
  text-align: left;
  transition: none;
}

.sidebar-selector-item:hover:not(:disabled),
.sidebar-selector-item:focus-visible {
  background: var(--btn-primary-bg);
  color: #fff;
  outline: 0;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.sidebar-selector-item--active {
  background: var(--btn-active-bg);
  box-shadow: var(--btn-active-shadow);
}

.sidebar-selector-item--disabled {
  cursor: default;
  opacity: 0.52;
}

.sidebar-selector-item-copy {
  display: grid;
  min-width: 0;
  gap: 0;
}

.sidebar-selector-item-copy > span {
  overflow: hidden;
  font-size: 13px;
  font-weight: 500;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sidebar-selector-item-copy small {
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 560;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sidebar-selector-empty {
  margin: 0;
  padding: 4px 10px 7px;
  color: var(--text-faint);
  font-size: 12px;
  font-weight: 560;
}

.sidebar-selector-actions {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 2px;
  padding-top: 4px;
  margin-top: 4px;
  border-top: 1px solid var(--hairline);
  background: transparent;
}

.sidebar-selector-actions button {
  display: inline-flex;
  min-width: 0;
  height: 28px;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 0 8px;
  border-radius: 8px;
  background: transparent;
  color: var(--text);
  cursor: pointer;
  font-size: 12px;
  font-weight: 500;
  letter-spacing: 0;
  transition: none;
}

.sidebar-selector-actions button:hover,
.sidebar-selector-actions button:focus-visible {
  background: var(--btn-primary-bg);
  color: #fff;
  outline: 0;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.sidebar-selector-actions button span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sidebar-selector-popover-enter-active,
.sidebar-selector-popover-leave-active {
  transition: opacity 130ms cubic-bezier(0.2, 0, 0, 1), transform 130ms cubic-bezier(0.2, 0, 0, 1);
}

.sidebar-selector-popover-enter-from,
.sidebar-selector-popover-leave-to {
  opacity: 0;
  transform: scale(0.93);
}
</style>
