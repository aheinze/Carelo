<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue';
import AppIcon from './AppIcon.vue';
import { useDialog } from '../composables/useDialog';
import { useFileManagerStore } from '../stores/fileManagerStore';

const props = defineProps({
  placement: {
    type: String,
    default: 'below',
  },
  variant: {
    type: String,
    default: 'toolbar',
  },
});

const store = useFileManagerStore();
const dialog = useDialog();
const selectorRef = ref(null);
const popoverRef = ref(null);
const open = ref(false);
const popoverPositioned = ref(false);
const popoverPlacement = ref(props.placement);
const popoverStyle = ref({
  top: '0px',
  left: '0px',
  width: '280px',
});

const activeWorkspaceLabel = computed(() => store.activeWorkspace?.name || 'Workspace');
const workspaceCountLabel = computed(() => {
  const count = store.workspaces.length;
  return count === 1 ? '1 saved workspace' : `${count} saved workspaces`;
});

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function updatePopoverPosition() {
  if (!open.value || !selectorRef.value || !popoverRef.value) {
    return;
  }

  const triggerRect = selectorRef.value.getBoundingClientRect();
  const popoverRect = popoverRef.value.getBoundingClientRect();
  const viewportWidth = window.innerWidth || document.documentElement.clientWidth || 0;
  const viewportHeight = window.innerHeight || document.documentElement.clientHeight || 0;
  const padding = 8;
  const gap = props.placement === 'above' ? 8 : 10;
  const preferredWidth = props.placement === 'above' ? 286 : 306;
  const width = Math.min(preferredWidth, Math.max(180, viewportWidth - (padding * 2)));
  const height = popoverRect.height || 220;
  const canOpenAbove = triggerRect.top >= height + gap + padding;
  const nextPlacement = props.placement === 'above' && canOpenAbove ? 'above' : 'below';
  const maxLeft = Math.max(padding, viewportWidth - width - padding);
  const left = clamp(triggerRect.left, padding, maxLeft);
  const top = nextPlacement === 'above'
    ? Math.max(padding, triggerRect.top - height - gap)
    : clamp(triggerRect.bottom + gap, padding, Math.max(padding, viewportHeight - height - padding));

  popoverPlacement.value = nextPlacement;
  popoverStyle.value = {
    top: `${top}px`,
    left: `${left}px`,
    width: `${width}px`,
  };
}

async function openPopover() {
  popoverPositioned.value = false;
  open.value = true;
  await nextTick();
  updatePopoverPosition();
  popoverPositioned.value = true;
}

function togglePopover(event) {
  event?.stopPropagation();

  if (open.value) {
    closePopover();
    return;
  }

  openPopover();
}

function closePopover() {
  open.value = false;
  popoverPositioned.value = false;
}

function workspaceTabSummary(workspace) {
  const leftCount = workspace?.left?.tabs?.length || 0;
  const rightCount = workspace?.right?.tabs?.length || 0;
  const total = leftCount + rightCount;
  const tabLabel = total === 1 ? 'tab' : 'tabs';
  const activePaneLabel = workspace?.activePaneId === 'left' ? 'left active' : 'right active';

  return `${total} ${tabLabel} · ${activePaneLabel}`;
}

async function createWorkspace() {
  closePopover();

  const suggestedName = `Workspace ${store.workspaces.length + 1}`;
  const name = (await dialog.prompt({
    title: 'New Workspace',
    icon: 'briefcase',
    message: 'Save the current tabs in both panes.',
    inputLabel: 'Name',
    inputValue: suggestedName,
    inputRequired: true,
    confirmLabel: 'Create',
  }))?.trim();

  if (!name) {
    return;
  }

  store.saveCurrentWorkspace(name, { updateExisting: false });
}

async function selectWorkspace(workspaceId) {
  if (!workspaceId) {
    return;
  }

  const applied = await store.applyWorkspace(workspaceId);

  if (applied) {
    closePopover();
  }
}

async function updateWorkspace(workspace, event) {
  event?.stopPropagation();

  if (!workspace?.id) {
    return;
  }

  closePopover();

  const confirmed = await dialog.confirm({
    title: 'Update Workspace',
    icon: 'sync',
    message: `Update "${workspace.name}" with the current open tabs?`,
    detail: 'This replaces its saved tab layout with the current pane state.',
    confirmLabel: 'Update',
  });

  if (!confirmed) {
    return;
  }

  store.updateWorkspaceFromCurrent(workspace.id);
}

async function renameWorkspace(workspace, event) {
  event?.stopPropagation();

  if (!workspace?.id) {
    return;
  }

  closePopover();

  const name = (await dialog.prompt({
    title: 'Rename Workspace',
    icon: 'pencil',
    inputLabel: 'Name',
    inputValue: workspace.name,
    inputRequired: true,
    confirmLabel: 'Rename',
  }))?.trim();

  if (!name || name === workspace.name) {
    return;
  }

  store.renameWorkspace(workspace.id, name);
}

async function deleteWorkspace(workspace, event) {
  event?.stopPropagation();

  if (!workspace?.id) {
    return;
  }

  closePopover();

  const confirmed = await dialog.confirm({
    title: 'Delete Workspace',
    icon: 'trash',
    message: `Delete "${workspace.name}"?`,
    detail: 'The saved workspace will be removed. Open tabs stay unchanged.',
    confirmLabel: 'Delete',
    variant: 'danger',
    destructive: true,
  });

  if (!confirmed) {
    return;
  }

  store.removeWorkspace(workspace.id);
}

function handleDocumentPointerDown(event) {
  if (
    !open.value
    || selectorRef.value?.contains(event.target)
    || popoverRef.value?.contains(event.target)
  ) {
    return;
  }

  closePopover();
}

function handleKeydown(event) {
  if (event.key === 'Escape' && open.value) {
    closePopover();
  }
}

onMounted(() => {
  document.addEventListener('pointerdown', handleDocumentPointerDown, true);
  window.addEventListener('resize', updatePopoverPosition);
  window.addEventListener('scroll', updatePopoverPosition, true);
  window.addEventListener('keydown', handleKeydown);
});

onUnmounted(() => {
  document.removeEventListener('pointerdown', handleDocumentPointerDown, true);
  window.removeEventListener('resize', updatePopoverPosition);
  window.removeEventListener('scroll', updatePopoverPosition, true);
  window.removeEventListener('keydown', handleKeydown);
});
</script>

<template>
  <div
    ref="selectorRef"
    class="workspace-selector"
    :class="{
      'workspace-selector--open': open,
      'workspace-selector--above': placement === 'above',
      'workspace-selector--sidebar': variant === 'sidebar',
    }"
    aria-label="Workspaces"
    @mousedown.stop
    @dblclick.stop
  >
    <button
      type="button"
      class="workspace-trigger"
      aria-haspopup="menu"
      :aria-expanded="open"
      aria-label="Open workspaces"
      @click="togglePopover"
      @keydown.stop
    >
      <AppIcon name="briefcase" :size="14" :stroke-width="1.85" class="workspace-trigger-icon" />
      <span class="workspace-trigger-label">{{ activeWorkspaceLabel }}</span>
      <AppIcon name="chevron-down" :size="12" :stroke-width="2.1" class="workspace-trigger-chevron" />
    </button>

    <Teleport to="body">
      <Transition name="workspace-popover">
        <div
          v-if="open"
          ref="popoverRef"
          class="workspace-popover"
          :class="{ 'workspace-popover--above': popoverPlacement === 'above' }"
          :style="{
            ...popoverStyle,
            visibility: open && !popoverPositioned ? 'hidden' : 'visible',
          }"
          role="menu"
          aria-label="Saved workspaces"
          @click.stop
          @mousedown.stop
        >
          <header class="workspace-popover-header">
            <strong>Workspaces</strong>
            <span>{{ workspaceCountLabel }}</span>
          </header>

          <div v-if="store.workspaces.length > 0" class="workspace-list">
            <div
              v-for="workspace in store.workspaces"
              :key="workspace.id"
              class="workspace-item"
              role="none"
              :class="{ 'workspace-item--active': store.activeWorkspaceId === workspace.id }"
            >
              <button
                type="button"
                role="menuitemradio"
                class="workspace-item-select"
                :aria-checked="store.activeWorkspaceId === workspace.id"
                @click="selectWorkspace(workspace.id)"
              >
                <span class="workspace-item-icon" aria-hidden="true">
                  <AppIcon name="briefcase" :size="15" :stroke-width="1.85" />
                </span>
                <span class="workspace-item-copy">
                  <span>{{ workspace.name }}</span>
                  <small>{{ workspaceTabSummary(workspace) }}</small>
                </span>
                <AppIcon
                  v-if="store.activeWorkspaceId === workspace.id"
                  name="check"
                  :size="14"
                  :stroke-width="2.1"
                  class="workspace-item-check"
                />
              </button>
              <button
                v-tooltip="'Update workspace'"
                type="button"
                class="workspace-item-action"
                :aria-label="`Update ${workspace.name} workspace from current tabs`"
                @click.stop="updateWorkspace(workspace, $event)"
                @keydown.stop
              >
                <AppIcon name="sync" :size="14" :stroke-width="1.9" />
              </button>
              <button
                v-tooltip="'Rename workspace'"
                type="button"
                class="workspace-item-action"
                :aria-label="`Rename ${workspace.name} workspace`"
                @click.stop="renameWorkspace(workspace, $event)"
                @keydown.stop
              >
                <AppIcon name="pencil" :size="14" :stroke-width="1.9" />
              </button>
              <button
                v-tooltip="'Delete workspace'"
                type="button"
                class="workspace-item-action workspace-item-action--danger"
                :aria-label="`Delete ${workspace.name} workspace`"
                @click.stop="deleteWorkspace(workspace, $event)"
                @keydown.stop
              >
                <AppIcon name="trash" :size="14" :stroke-width="1.9" />
              </button>
            </div>
          </div>

          <p v-else class="workspace-empty">No saved workspaces</p>

          <div class="workspace-actions">
            <button type="button" role="menuitem" class="workspace-create" @click="createWorkspace">
              <AppIcon name="briefcase-plus" :size="15" :stroke-width="1.9" />
              <span>New Workspace</span>
            </button>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>

<style scoped>
.workspace-selector {
  position: relative;
  display: flex;
  width: auto;
  min-width: 120px;
  max-width: 168px;
  height: 34px;
  flex: 0 1 152px;
  align-items: center;
  overflow: visible;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--icon);
  transition: border-color 140ms ease, box-shadow 140ms ease, background 140ms ease;
}

.workspace-selector--sidebar {
  width: fit-content;
  min-width: 0;
  max-width: 168px;
  height: 30px;
  flex: 0 1 auto;
  border-color: transparent;
  border-radius: 7px;
  background: transparent;
  box-shadow: none;
}

.workspace-selector:hover {
  background:
    linear-gradient(180deg, rgb(255 255 255 / 0.055), rgb(255 255 255 / 0.012)),
    var(--input-bg);
}

.workspace-selector--sidebar:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.workspace-selector--open,
.workspace-selector:focus-within {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.workspace-selector--sidebar.workspace-selector--open,
.workspace-selector--sidebar:focus-within {
  border-color: transparent;
  background: var(--btn-hover);
  color: var(--text);
  box-shadow: none;
}

.workspace-trigger {
  display: inline-grid;
  height: 100%;
  min-width: 0;
  flex: 1 1 auto;
  grid-template-columns: auto minmax(0, 1fr) auto;
  align-items: center;
  gap: 8px;
  border-radius: 7px;
  padding: 0 8px 0 9px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  text-align: left;
  transition: color 80ms ease;
}

.workspace-trigger:hover {
  color: var(--text);
}

.workspace-selector--sidebar .workspace-trigger {
  grid-template-columns: auto minmax(0, max-content);
  width: fit-content;
  max-width: 100%;
  padding: 0 7px;
}

.workspace-trigger-label {
  min-width: 0;
  overflow: hidden;
  font-size: 13px;
  font-weight: 650;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.workspace-selector--sidebar .workspace-trigger-label {
  max-width: 124px;
  font-size: 12.5px;
  font-weight: 610;
  text-align: right;
}

.workspace-trigger-icon,
.workspace-trigger-chevron {
  flex: 0 0 auto;
}

.workspace-selector--sidebar .workspace-trigger-chevron {
  display: none;
}

.workspace-popover {
  position: fixed;
  z-index: 2400;
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 12px;
  padding: 5px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  transform-origin: top left;
}

.workspace-popover--above {
  transform-origin: bottom left;
}

.workspace-popover-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 6px 10px 8px;
  border-bottom: 1px solid var(--hairline);
}

.workspace-popover-header strong {
  min-width: 0;
  overflow: hidden;
  font-size: 12px;
  font-weight: 700;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.workspace-popover-header span {
  flex: 0 0 auto;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
}

.workspace-list {
  display: grid;
  max-height: min(330px, calc(100vh - 108px));
  overflow-y: auto;
  overscroll-behavior: contain;
  padding-top: 5px;
  scrollbar-width: thin;
  scrollbar-color: var(--control-border) transparent;
}

.workspace-list::-webkit-scrollbar {
  width: 9px;
}

.workspace-list::-webkit-scrollbar-track {
  background: transparent;
}

.workspace-list::-webkit-scrollbar-thumb {
  border: 2px solid transparent;
  border-radius: 999px;
  background: var(--control-border);
  background-clip: padding-box;
}

.workspace-item {
  display: grid;
  width: 100%;
  grid-template-columns: minmax(0, 1fr) repeat(3, 30px);
  align-items: center;
  gap: 2px;
  border-radius: 8px;
  background: transparent;
}

.workspace-item-select {
  display: grid;
  width: 100%;
  min-height: 42px;
  grid-template-columns: 18px minmax(0, 1fr) 16px;
  align-items: center;
  gap: 9px;
  border-radius: 8px;
  padding: 5px 9px;
  background: transparent;
  color: var(--text);
  cursor: pointer;
  text-align: left;
  transition: none;
}

.workspace-item-select:hover,
.workspace-item-select:focus-visible {
  background: var(--btn-primary-bg);
  color: #fff;
  outline: 0;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.workspace-item--active .workspace-item-select {
  background: var(--btn-active-bg);
  box-shadow: var(--btn-active-shadow);
}

.workspace-item-icon,
.workspace-item-check {
  color: currentColor;
}

.workspace-item-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.workspace-item-copy > span {
  overflow: hidden;
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.workspace-item-copy small {
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 560;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.workspace-item-select:hover .workspace-item-copy small,
.workspace-item-select:focus-visible .workspace-item-copy small {
  color: rgb(255 255 255 / 0.78);
}

.workspace-item-action {
  display: inline-grid;
  width: 28px;
  height: 34px;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--text-faint);
  cursor: pointer;
  opacity: 0.64;
  transition: background 80ms ease, color 80ms ease, opacity 80ms ease;
}

.workspace-item-action:hover,
.workspace-item-action:focus-visible {
  background: var(--btn-hover);
  color: var(--text);
  opacity: 1;
  outline: 0;
}

.workspace-item-action--danger:hover,
.workspace-item-action--danger:focus-visible {
  background: color-mix(in srgb, var(--danger) 14%, transparent);
  color: var(--danger);
}

.workspace-empty {
  margin: 0;
  padding: 12px 10px 11px;
  color: var(--text-faint);
  font-size: 12px;
  font-weight: 560;
}

.workspace-actions {
  margin-top: 5px;
  padding-top: 5px;
  border-top: 1px solid var(--hairline);
}

.workspace-create {
  display: inline-flex;
  width: 100%;
  height: 32px;
  align-items: center;
  justify-content: center;
  gap: 7px;
  border-radius: 8px;
  background: transparent;
  color: var(--text);
  cursor: pointer;
  font-size: 12px;
  font-weight: 620;
  letter-spacing: 0;
  transition: none;
}

.workspace-create:hover,
.workspace-create:focus-visible {
  background: var(--btn-primary-bg);
  color: #fff;
  outline: 0;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.workspace-popover-enter-active,
.workspace-popover-leave-active {
  transition: opacity 130ms cubic-bezier(0.2, 0, 0, 1), transform 130ms cubic-bezier(0.2, 0, 0, 1);
}

.workspace-popover-enter-from,
.workspace-popover-leave-to {
  opacity: 0;
  transform: scale(0.96);
}
</style>
