<script setup>
import { computed, defineAsyncComponent, nextTick, ref, onMounted, onUnmounted } from 'vue';
import AppIcon from './AppIcon.vue';
import WorkspaceSelector from './WorkspaceSelector.vue';
import { getFileMetadata, removeRemoteVolume } from '../composables/useFileOperations';
import { useDialog } from '../composables/useDialog';
import { useFileManagerStore } from '../stores/fileManagerStore';
import {
  CREATE_SIDEBAR_GROUP_EVENT,
  OPEN_REMOTE_STORAGE_EVENT,
} from '../utils/appEvents';
import {
  closeTauriWindow,
  getTauriWindow,
  minimizeTauriWindow,
  toggleMaximizeTauriWindow,
} from '../composables/useTauriWindow';

const FILE_DRAG_MIME = 'application/x-carelo-files';
const FAVORITE_DRAG_MIME = 'application/x-carelo-favorite';
const DEFAULT_FAVORITE_GROUP_ID = 'favorites';
const RemoteVolumeModal = defineAsyncComponent(() => import('./RemoteVolumeModal.vue'));

const store = useFileManagerStore();
const dialog = useDialog();
const remoteModalVisible = ref(false);
const sidebarFooter = ref(null);
const sidebarAddMenuOpen = ref(false);
const sidebarItemMenu = ref(null);
const sidebarItemMenuRef = ref(null);
const sidebarItemMenuStyle = ref({
  left: '0px',
  top: '0px',
  maxHeight: 'calc(100vh - 16px)',
});
const draggedFavoriteId = ref(null);
const favoriteDropIndex = ref(null);
const favoriteDropGroupId = ref('');
const mountingDevicePath = ref('');
const deviceBusyAction = ref('');
let volumeRefreshTimer = null;

const favoriteGroupsForMenu = computed(() => {
  if (store.favoriteGroups.length > 0) {
    return store.favoriteGroups;
  }

  return [{
    id: DEFAULT_FAVORITE_GROUP_ID,
    name: 'Favorites',
  }];
});

const sidebarItemMenuTitle = computed(() => sidebarItemMenu.value?.item?.name || 'Sidebar Item');
const sidebarItemMenuDetail = computed(() => {
  const item = sidebarItemMenu.value?.item;

  if (!item) {
    return '';
  }

  if (item.isFavorite) {
    return 'Sidebar Shortcut';
  }

  if (item.isRemote) {
    return 'Remote Storage';
  }

  if (item.isRecent) {
    return 'Recent';
  }

  if (item.needsUnlock) {
    return 'Encrypted Device';
  }

  if (item.isMountable) {
    return 'Device';
  }

  return item.detail || 'Location';
});
const sidebarItemMenuGroups = computed(() => {
  const item = sidebarItemMenu.value?.item;

  if (!item) {
    return [];
  }

  const groups = [];
  const canOpen = Boolean(item.path || item.isMountable);
  const openDisabled = item.disabled || (item.devicePath && mountingDevicePath.value === item.devicePath);

  groups.push({
    id: 'open',
    items: [
      {
        id: 'open',
        action: 'open',
        label: item.needsUnlock ? 'Unlock and Open' : item.isMountable ? 'Mount and Open' : 'Open',
        icon: item.icon || (item.needsUnlock ? 'lock' : item.isMountable ? 'drive' : 'folder'),
        disabled: !canOpen || openDisabled,
      },
      {
        id: 'openInNewTab',
        action: 'openInNewTab',
        label: 'Open in New Tab',
        icon: 'plus',
        disabled: !item.path || item.disabled,
      },
      {
        id: 'copyPath',
        action: 'copyPath',
        label: 'Copy Path',
        icon: 'copy',
        disabled: !item.path,
      },
    ],
  });

  if (item.isFavorite) {
    const currentGroupId = favoriteGroupIdForItem(item);
    const moveItems = favoriteGroupsForMenu.value.map((group) => {
      const isCurrentGroup = group.id === currentGroupId;

      return {
        id: `move:${group.id}`,
        action: 'moveFavoriteToGroup',
        targetGroupId: group.id,
        label: group.name,
        icon: isCurrentGroup ? 'check' : 'folder',
        disabled: isCurrentGroup,
      };
    });

    moveItems.push({
      id: 'move:new',
      action: 'moveFavoriteToNewGroup',
      label: 'New Group...',
      icon: 'folder-plus',
    });

    groups.push({
      id: 'move',
      label: 'Move to Group',
      items: moveItems,
    });

    groups.push({
      id: 'favorite-danger',
      items: [
        {
          id: 'removeFavorite',
          action: 'removeFavorite',
          label: 'Remove from Sidebar',
          icon: 'x',
          danger: true,
        },
      ],
    });
  }

  if (item.isRemote) {
    groups.push({
      id: 'remote',
      items: [
        {
          id: 'refreshRemote',
          action: 'refreshRemote',
          label: 'Check Connection',
          icon: 'refresh',
          disabled: !item.remoteId,
        },
        {
          id: 'disconnectRemote',
          action: 'disconnectRemote',
          label: 'Disconnect',
          icon: 'x',
          danger: true,
          disabled: !item.remoteId,
        },
      ],
    });
  }

  if (item.canEject) {
    groups.push({
      id: 'device',
      items: [
        {
          id: 'ejectDevice',
          action: 'ejectDevice',
          label: 'Eject',
          icon: 'eject',
          disabled: item.devicePath && mountingDevicePath.value === item.devicePath,
        },
      ],
    });
  }

  if (item.isRecent) {
    groups.push({
      id: 'recent',
      items: [
        {
          id: 'removeRecent',
          action: 'removeRecent',
          label: 'Remove from Recent',
          icon: 'x',
          danger: true,
        },
      ],
    });
  }

  return groups
    .map((group) => ({
      ...group,
      items: group.items.filter((menuItem) => !menuItem.disabled),
    }))
    .filter((group) => group.items.length > 0);
});

async function openSidebarItem(item) {
  if (item.disabled || (item.devicePath && mountingDevicePath.value === item.devicePath)) {
    return;
  }

  if (item.isMountable && item.devicePath) {
    await mountSidebarVolume(item);
    return;
  }

  if (item.path) {
    store.setPanePath(store.activePaneId, item.path);
  }
}

async function mountSidebarVolume(item) {
  mountingDevicePath.value = item.devicePath;
  deviceBusyAction.value = item.needsUnlock ? 'unlock' : 'mount';

  try {
    const volume = await store.mountLocalVolume(item);
    await store.refreshVolumes();

    if (volume?.path) {
      store.setPanePath(store.activePaneId, volume.path);
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
    deviceBusyAction.value = '';
  }
}

function deviceBusyLabel(item) {
  if (deviceBusyAction.value === 'eject') {
    return 'Ejecting…';
  }

  return item?.needsUnlock ? 'Unlocking…' : 'Mounting…';
}

function remoteIdFromPath(path) {
  return path?.startsWith('remote://') ? path.slice('remote://'.length).split('/')[0] : '';
}

function openRemoteModal() {
  closeSidebarAddMenu();
  remoteModalVisible.value = true;
}

function closeRemoteModal() {
  remoteModalVisible.value = false;
}

function dataTransferTypes(event) {
  return Array.from(event?.dataTransfer?.types || []);
}

function hasDataTransferType(event, type) {
  return dataTransferTypes(event).includes(type);
}

function isFavoriteGroupSection(section) {
  return Boolean(section?.isFavoriteGroup);
}

function favoriteGroupIdForSection(section) {
  return section?.favoriteGroupId || DEFAULT_FAVORITE_GROUP_ID;
}

function favoriteGroupIdForItem(item) {
  return item?.favoriteGroupId || item?.groupId || DEFAULT_FAVORITE_GROUP_ID;
}

function favoriteGroupItems(groupId) {
  return store.favorites.filter((favorite) =>
    (favorite.groupId || DEFAULT_FAVORITE_GROUP_ID) === groupId,
  );
}

function favoriteCountForGroup(groupId) {
  return favoriteGroupItems(groupId).length;
}

function favoriteIndexForItem(item) {
  const groupId = favoriteGroupIdForItem(item);
  return favoriteGroupItems(groupId).findIndex((favorite) => favorite.id === item.id);
}

function isFavoriteDropTarget(section) {
  return (
    isFavoriteGroupSection(section) &&
    favoriteDropIndex.value !== null &&
    favoriteDropGroupId.value === favoriteGroupIdForSection(section)
  );
}

function isFavoriteDropTargetEnd(section) {
  return (
    isFavoriteDropTarget(section) &&
    favoriteDropIndex.value === favoriteCountForGroup(favoriteGroupIdForSection(section))
  );
}

function isFavoriteItemDropBefore(item) {
  return (
    item.isFavorite &&
    favoriteDropGroupId.value === favoriteGroupIdForItem(item) &&
    favoriteDropIndex.value === favoriteIndexForItem(item)
  );
}

function readFavoriteDragPayload(event) {
  const rawPayload = event?.dataTransfer?.getData(FAVORITE_DRAG_MIME);

  if (!rawPayload) {
    return null;
  }

  try {
    const payload = JSON.parse(rawPayload);
    return payload?.id ? payload : null;
  } catch {
    return null;
  }
}

function readFileDragPayload(event) {
  if (store.dragOperation?.entries?.length) {
    return store.dragOperation;
  }

  const rawPayload = event?.dataTransfer?.getData(FILE_DRAG_MIME);

  if (!rawPayload) {
    return null;
  }

  try {
    const payload = JSON.parse(rawPayload);
    return Array.isArray(payload.entries) ? payload : null;
  } catch {
    return null;
  }
}

function draggedDirectoryEntriesFromStoreOrPayload(event, payload = readFileDragPayload(event)) {
  return (payload?.entries || []).filter((entry) => entry.kind === 'directory');
}

function isPotentialFileDropEvent(event) {
  return Boolean(event?.dataTransfer);
}

function isFavoriteDropEvent(event) {
  return hasDataTransferType(event, FAVORITE_DRAG_MIME) || isPotentialFileDropEvent(event);
}

function nameFromPath(path) {
  const value = String(path || '').replace(/\/+$/, '');

  if (!value || value === '/' || value === '~') {
    return value || '';
  }

  if (value.startsWith('remote://')) {
    const parts = value.slice('remote://'.length).split('/').filter(Boolean);
    return parts.at(-1) || parts[0] || value;
  }

  return value.split('/').filter(Boolean).at(-1) || value;
}

function pathFromDroppedTextLine(line) {
  const value = String(line || '').trim();

  if (!value || value.startsWith('#')) {
    return '';
  }

  if (value.startsWith('file://')) {
    try {
      return decodeURIComponent(new URL(value).pathname);
    } catch {
      return decodeURIComponent(value.replace(/^file:\/\//, ''));
    }
  }

  return value.startsWith('/') || value.startsWith('~/') || value === '~' || value.startsWith('remote://')
    ? value
    : '';
}

function droppedTextPaths(event) {
  const text = event.dataTransfer?.getData('text/uri-list')
    || event.dataTransfer?.getData('text/plain')
    || '';

  return [...new Set(
    text
      .split(/\r?\n/)
      .map(pathFromDroppedTextLine)
      .filter(Boolean),
  )];
}

async function directoryEntriesFromDrop(event, payload = readFileDragPayload(event)) {
  const payloadEntries = draggedDirectoryEntriesFromStoreOrPayload(event, payload);

  if (payloadEntries.length > 0) {
    return payloadEntries;
  }

  const paths = droppedTextPaths(event);
  const settled = await Promise.allSettled(paths.map((path) => getFileMetadata(path)));

  return settled
    .filter((result) => result.status === 'fulfilled' && result.value?.kind === 'directory')
    .map((result) => ({
      name: nameFromPath(result.value.path),
      path: result.value.path,
      kind: result.value.kind,
    }));
}

function setFavoriteDropTarget(groupId, index) {
  const targetGroupId = groupId || DEFAULT_FAVORITE_GROUP_ID;
  const nextIndex = Math.max(0, Math.min(favoriteCountForGroup(targetGroupId), Number(index) || 0));

  if (favoriteDropGroupId.value !== targetGroupId) {
    favoriteDropGroupId.value = targetGroupId;
  }

  if (favoriteDropIndex.value !== nextIndex) {
    favoriteDropIndex.value = nextIndex;
  }
}

function clearFavoriteDragState() {
  draggedFavoriteId.value = null;
  favoriteDropIndex.value = null;
  favoriteDropGroupId.value = '';
}

function isDragPointInsideElement(event, element) {
  if (!element || typeof event.clientX !== 'number' || typeof event.clientY !== 'number') {
    return false;
  }

  const rect = element.getBoundingClientRect();

  return (
    event.clientX >= rect.left &&
    event.clientX <= rect.right &&
    event.clientY >= rect.top &&
    event.clientY <= rect.bottom
  );
}

function favoriteDropTargetForEvent(item, event) {
  const groupId = favoriteGroupIdForItem(item);
  const index = favoriteIndexForItem(item);

  if (index < 0) {
    return {
      groupId,
      index: favoriteCountForGroup(groupId),
    };
  }

  const rect = event.currentTarget.getBoundingClientRect();
  return {
    groupId,
    index: event.clientY < rect.top + rect.height / 2 ? index : index + 1,
  };
}

function updateSidebarItemMenuPosition() {
  nextTick(() => {
    const menu = sidebarItemMenuRef.value;
    const position = sidebarItemMenu.value?.position;

    if (!menu || !position) {
      return;
    }

    const margin = 8;
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const rect = menu.getBoundingClientRect();
    const maxHeight = Math.max(1, viewportHeight - margin * 2);
    const menuHeight = Math.min(menu.scrollHeight, maxHeight);
    const wouldOverflowRight = position.x + rect.width + margin > viewportWidth;
    const wouldOverflowBottom = position.y + menuHeight + margin > viewportHeight;
    const left = Math.min(
      Math.max(margin, wouldOverflowRight ? position.x - rect.width : position.x),
      Math.max(margin, viewportWidth - rect.width - margin),
    );
    const top = Math.min(
      Math.max(margin, wouldOverflowBottom ? position.y - menuHeight : position.y),
      Math.max(margin, viewportHeight - menuHeight - margin),
    );

    sidebarItemMenuStyle.value = {
      left: `${left}px`,
      top: `${top}px`,
      maxHeight: `${maxHeight}px`,
    };
  });
}

function openSidebarItemContextMenu(section, item, event) {
  event?.preventDefault?.();
  event?.stopPropagation?.();
  closeSidebarAddMenu();
  sidebarItemMenu.value = {
    section,
    item,
    position: {
      x: event?.clientX ?? 0,
      y: event?.clientY ?? 0,
    },
  };
  sidebarItemMenuStyle.value = {
    left: `${event?.clientX ?? 0}px`,
    top: `${event?.clientY ?? 0}px`,
    maxHeight: 'calc(100vh - 16px)',
  };
  updateSidebarItemMenuPosition();
  focusFirstSidebarContextMenuItem();
}

function openSidebarItemKeyboardContextMenu(section, item, event) {
  const rect = event.currentTarget.getBoundingClientRect();

  openSidebarItemContextMenu(section, item, {
    preventDefault: () => event.preventDefault(),
    stopPropagation: () => event.stopPropagation(),
    clientX: rect.left + Math.min(rect.width - 12, 32),
    clientY: rect.top + Math.min(rect.height - 4, 28),
  });
}

function closeSidebarItemContextMenu() {
  sidebarItemMenu.value = null;
}

function focusFirstSidebarContextMenuItem() {
  nextTick(() => {
    sidebarItemMenuRef.value?.querySelector('.sidebar-context-menu-item:not(:disabled)')?.focus?.({
      preventScroll: true,
    });
  });
}

function sidebarContextMenuItems() {
  return Array.from(sidebarItemMenuRef.value?.querySelectorAll('.sidebar-context-menu-item:not(:disabled)') || []);
}

function focusSidebarContextMenuItemAt(index) {
  const items = sidebarContextMenuItems();

  if (items.length === 0) {
    return;
  }

  const nextIndex = (index + items.length) % items.length;
  items[nextIndex]?.focus?.({ preventScroll: true });
}

function focusRelativeSidebarContextMenuItem(delta) {
  const items = sidebarContextMenuItems();

  if (items.length === 0) {
    return;
  }

  const currentIndex = items.indexOf(document.activeElement);
  focusSidebarContextMenuItemAt(currentIndex < 0 ? (delta < 0 ? items.length - 1 : 0) : currentIndex + delta);
}

function handleSidebarItemContextKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault();
    closeSidebarItemContextMenu();
    return;
  }

  if (event.key === 'ArrowDown') {
    event.preventDefault();
    focusRelativeSidebarContextMenuItem(1);
    return;
  }

  if (event.key === 'ArrowUp') {
    event.preventDefault();
    focusRelativeSidebarContextMenuItem(-1);
    return;
  }

  if (event.key === 'Home') {
    event.preventDefault();
    focusSidebarContextMenuItemAt(0);
    return;
  }

  if (event.key === 'End') {
    event.preventDefault();
    focusSidebarContextMenuItemAt(-1);
  }
}

function handleSidebarItemKeydown(section, item, event) {
  if ((event.key === 'F10' && event.shiftKey) || event.key === 'ContextMenu') {
    openSidebarItemKeyboardContextMenu(section, item, event);
  }
}

function copySidebarPathToClipboard(path) {
  navigator.clipboard?.writeText(path).catch(() => {});
}

async function moveFavoriteItemToGroup(item, targetGroupId) {
  if (!item?.isFavorite || !targetGroupId || favoriteGroupIdForItem(item) === targetGroupId) {
    return;
  }

  await store.moveFavorite(item.id, favoriteCountForGroup(targetGroupId), targetGroupId);
}

async function moveFavoriteItemToNewGroup(item) {
  if (!item?.isFavorite) {
    return;
  }

  const name = (await dialog.prompt({
    title: 'New Sidebar Group',
    message: 'Create a section and move this shortcut into it.',
    inputLabel: 'Group name',
    inputValue: 'New Group',
    inputRequired: true,
    confirmLabel: 'Create and Move',
    icon: 'folder-plus',
  }))?.trim();

  if (!name) {
    return;
  }

  const group = await store.addFavoriteGroup(name);
  await moveFavoriteItemToGroup(item, group.id);
}

async function runSidebarItemContextAction(menuItem) {
  if (!menuItem || menuItem.disabled) {
    return;
  }

  const item = sidebarItemMenu.value?.item;
  closeSidebarItemContextMenu();

  if (!item) {
    return;
  }

  try {
    if (menuItem.action === 'open') {
      await openSidebarItem(item);
      return;
    }

    if (menuItem.action === 'openInNewTab') {
      if (item.path && !item.disabled) {
        store.addPaneTab(store.activePaneId, item.path);
      }
      return;
    }

    if (menuItem.action === 'copyPath') {
      if (item.path) {
        copySidebarPathToClipboard(item.path);
      }
      return;
    }

    if (menuItem.action === 'moveFavoriteToGroup') {
      await moveFavoriteItemToGroup(item, menuItem.targetGroupId);
      return;
    }

    if (menuItem.action === 'moveFavoriteToNewGroup') {
      await moveFavoriteItemToNewGroup(item);
      return;
    }

    if (menuItem.action === 'removeFavorite') {
      await store.removeFavorite(item.id);
      return;
    }

    if (menuItem.action === 'refreshRemote') {
      await refreshRemoteItem(item);
      return;
    }

    if (menuItem.action === 'disconnectRemote') {
      await disconnectRemoteItem(item);
      return;
    }

    if (menuItem.action === 'ejectDevice') {
      await ejectSidebarDevice(item);
      return;
    }

    if (menuItem.action === 'removeRecent') {
      store.removeRecentLocation(item.path);
    }
  } catch (error) {
    await dialog.alert({
      title: 'Sidebar Action Failed',
      message: error?.message || 'The sidebar action could not be completed.',
      variant: 'warning',
    });
  }
}

function closestFromElements(elements, selector) {
  for (const element of elements) {
    const match = element?.closest?.(selector);

    if (match) {
      return match;
    }
  }

  return null;
}

function pointerElements(event) {
  if (typeof document.elementsFromPoint === 'function') {
    return document.elementsFromPoint(event.clientX, event.clientY);
  }

  return [document.elementFromPoint(event.clientX, event.clientY)].filter(Boolean);
}

function hasActiveDirectoryFileDrag() {
  return (store.dragOperation?.entries || []).some((entry) => entry.kind === 'directory');
}

function favoriteDropTargetForPointerEvent(event) {
  if (!hasActiveDirectoryFileDrag()) {
    return null;
  }

  const elements = pointerElements(event);
  const favoriteZone = closestFromElements(elements, '[data-favorite-drop-zone]');

  if (!favoriteZone) {
    return null;
  }

  const groupId = favoriteZone.dataset.favoriteGroupId || DEFAULT_FAVORITE_GROUP_ID;
  const favoriteItem = closestFromElements(elements, '[data-favorite-index]');

  if (favoriteItem && favoriteZone.contains(favoriteItem)) {
    const index = Number(favoriteItem.dataset.favoriteIndex);

    if (Number.isInteger(index)) {
      const rect = favoriteItem.getBoundingClientRect();
      return {
        groupId,
        index: event.clientY < rect.top + rect.height / 2 ? index : index + 1,
      };
    }
  }

  return {
    groupId,
    index: favoriteCountForGroup(groupId),
  };
}

function handlePointerFileDragMove(event) {
  if (draggedFavoriteId.value) {
    return;
  }

  const nextTarget = favoriteDropTargetForPointerEvent(event);

  if (nextTarget === null) {
    favoriteDropIndex.value = null;
    favoriteDropGroupId.value = '';
    return;
  }

  setFavoriteDropTarget(nextTarget.groupId, nextTarget.index);
}

function clearPointerFileDragIndicator() {
  if (!draggedFavoriteId.value) {
    favoriteDropIndex.value = null;
    favoriteDropGroupId.value = '';
  }
}

function handleFavoriteDragStart(item, event) {
  if (!item.isFavorite || !event.dataTransfer) {
    return;
  }

  draggedFavoriteId.value = item.id;
  event.dataTransfer.effectAllowed = 'move';
  event.dataTransfer.dropEffect = 'move';
  event.dataTransfer.setData(FAVORITE_DRAG_MIME, JSON.stringify({ id: item.id }));
  event.dataTransfer.setData('text/plain', item.path);
}

function handleFavoriteDragEnd() {
  clearFavoriteDragState();
}

function handleFavoriteItemDragOver(item, event) {
  if (!item.isFavorite || !isFavoriteDropEvent(event)) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  event.dataTransfer.dropEffect = hasDataTransferType(event, FAVORITE_DRAG_MIME) ? 'move' : 'copy';
  const target = favoriteDropTargetForEvent(item, event);
  setFavoriteDropTarget(target.groupId, target.index);
}

function handleFavoriteSectionDragOver(section, event) {
  if (!isFavoriteGroupSection(section) || !isFavoriteDropEvent(event)) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  event.dataTransfer.dropEffect = hasDataTransferType(event, FAVORITE_DRAG_MIME) ? 'move' : 'copy';
  const groupId = favoriteGroupIdForSection(section);
  setFavoriteDropTarget(groupId, favoriteCountForGroup(groupId));
}

function handleFavoriteSectionDragLeave(section, event) {
  if (!isFavoriteGroupSection(section) || !isFavoriteDropEvent(event)) {
    return;
  }

  if (
    event.relatedTarget instanceof Node &&
    event.currentTarget.contains(event.relatedTarget)
  ) {
    return;
  }

  if (isDragPointInsideElement(event, event.currentTarget)) {
    return;
  }

  favoriteDropIndex.value = null;
  favoriteDropGroupId.value = '';
}

function handleSidebarFileDragOver(event) {
  if (hasDataTransferType(event, FAVORITE_DRAG_MIME) || !isPotentialFileDropEvent(event)) {
    return;
  }

  event.preventDefault();
  event.dataTransfer.dropEffect = 'copy';
  setFavoriteDropTarget(DEFAULT_FAVORITE_GROUP_ID, favoriteCountForGroup(DEFAULT_FAVORITE_GROUP_ID));
}

function handleSidebarFileDragLeave(event) {
  if (event.currentTarget.contains(event.relatedTarget)) {
    return;
  }

  favoriteDropIndex.value = null;
  favoriteDropGroupId.value = '';
}

async function handleSidebarFileDrop(event) {
  if (hasDataTransferType(event, FAVORITE_DRAG_MIME) || !isPotentialFileDropEvent(event)) {
    return;
  }

  await dropFavoriteAt(
    favoriteDropGroupId.value || DEFAULT_FAVORITE_GROUP_ID,
    favoriteDropIndex.value ?? favoriteCountForGroup(DEFAULT_FAVORITE_GROUP_ID),
    event,
  );
}

async function dropFavoriteAt(groupId, index, event) {
  const favoritePayload = readFavoriteDragPayload(event);
  const filePayload = favoritePayload ? null : readFileDragPayload(event);
  const targetGroupId = groupId || DEFAULT_FAVORITE_GROUP_ID;

  event.preventDefault();
  event.stopPropagation();

  try {
    if (favoritePayload?.id) {
      await store.moveFavorite(favoritePayload.id, index, targetGroupId);
      return;
    }

    if (!store.claimFileDrop(filePayload?.id)) {
      return;
    }

    const directories = await directoryEntriesFromDrop(event, filePayload);

    if (directories.length > 0) {
      await store.addFavoritesFromEntries(directories, index, targetGroupId);
    }
  } finally {
    store.clearFileDrag();
    clearFavoriteDragState();
  }
}

async function handleFavoriteItemDrop(item, event) {
  if (!item.isFavorite || !isFavoriteDropEvent(event)) {
    return;
  }

  const target = favoriteDropTargetForEvent(item, event);
  await dropFavoriteAt(target.groupId, target.index, event);
}

async function handleFavoriteSectionDrop(section, event) {
  if (!isFavoriteGroupSection(section) || !isFavoriteDropEvent(event)) {
    return;
  }

  const groupId = favoriteGroupIdForSection(section);
  await dropFavoriteAt(
    groupId,
    favoriteDropIndex.value ?? favoriteCountForGroup(groupId),
    event,
  );
}

async function removeFavoriteItem(item, event) {
  event?.stopPropagation?.();
  await store.removeFavorite(item.id);
}

async function removeFavoriteGroup(section, event) {
  event?.stopPropagation?.();

  if (!section?.favoriteGroupId || section.isDefaultFavoriteGroup) {
    return;
  }

  const itemCount = section.items.length;
  const confirmed = await dialog.confirm({
    title: 'Delete Group',
    message: itemCount > 0
      ? `Delete "${section.title}" and remove its ${itemCount === 1 ? 'shortcut' : `${itemCount} shortcuts`} from the sidebar?`
      : `Delete "${section.title}"?`,
    confirmLabel: 'Delete Group',
    variant: 'danger',
    destructive: true,
  });

  if (!confirmed) {
    return;
  }

  try {
    await store.removeFavoriteGroup(section.favoriteGroupId);
  } catch (error) {
    await dialog.alert({
      title: 'Group Not Deleted',
      message: error?.message || 'Unable to delete sidebar group.',
      variant: 'warning',
    });
  }
}

async function disconnectRemoteItem(item, event) {
  event?.stopPropagation?.();

  const remoteId = remoteIdFromPath(item.path);

  if (!remoteId) {
    return;
  }

  const confirmed = await dialog.confirm({
    title: 'Disconnect Remote',
    message: `Remove ${item.name} from the sidebar?`,
    confirmLabel: 'Disconnect',
    variant: 'warning',
  });

  if (!confirmed) {
    return;
  }

  try {
    await removeRemoteVolume(remoteId);
    await store.refreshVolumes();

    if (store.activePane?.currentPath?.startsWith(item.path)) {
      store.setPanePath(store.activePaneId, '~');
    }
  } catch (error) {
    await dialog.alert({
      title: 'Disconnect Failed',
      message: error?.message || 'Unable to disconnect remote volume.',
      variant: 'warning',
    });
  }
}

async function refreshRemoteItem(item, event) {
  event?.stopPropagation?.();

  if (!item.remoteId) {
    return;
  }

  await store.refreshRemoteHealth(item.remoteId);
}

async function ejectSidebarDevice(item, event) {
  event?.stopPropagation?.();

  if (!item?.devicePath || mountingDevicePath.value === item.devicePath) {
    return;
  }

  mountingDevicePath.value = item.devicePath;
  deviceBusyAction.value = 'eject';

  try {
    await store.ejectLocalVolume(item);
  } catch (error) {
    await store.refreshVolumes();
    await dialog.alert({
      title: 'Eject Failed',
      message: error?.message || `Unable to eject ${item.name}.`,
      detail: item.devicePath || '',
      variant: 'warning',
    });
  } finally {
    mountingDevicePath.value = '';
    deviceBusyAction.value = '';
  }
}

function closeSidebarAddMenu() {
  sidebarAddMenuOpen.value = false;
}

function toggleSidebarAddMenu(event) {
  event?.stopPropagation();
  closeSidebarItemContextMenu();
  sidebarAddMenuOpen.value = !sidebarAddMenuOpen.value;
}

function handleDocumentPointerDown(event) {
  if (sidebarItemMenu.value) {
    if (sidebarItemMenuRef.value?.contains(event.target)) {
      return;
    }

    closeSidebarItemContextMenu();
  }

  if (!sidebarAddMenuOpen.value) {
    return;
  }

  if (sidebarFooter.value?.contains(event.target)) {
    return;
  }

  closeSidebarAddMenu();
}

function handleWindowKeydown(event) {
  if (event.key !== 'Escape') {
    return;
  }

  if (sidebarItemMenu.value) {
    event.preventDefault();
    closeSidebarItemContextMenu();
    return;
  }

  if (sidebarAddMenuOpen.value) {
    event.preventDefault();
    closeSidebarAddMenu();
  }
}

function handleWindowResize() {
  if (sidebarItemMenu.value) {
    updateSidebarItemMenuPosition();
  }
}

async function createFavoriteGroup() {
  closeSidebarAddMenu();

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

function startDragging(event) {
  if (event.button !== 0 || event.detail > 1) return;
  event.preventDefault();
  getTauriWindow()?.startDragging().catch(() => {});
}

function minimizeWindow(event) {
  event?.stopPropagation();
  minimizeTauriWindow().catch(() => {});
}

function toggleMaximizeWindow(event) {
  event?.stopPropagation();
  toggleMaximizeTauriWindow().catch(() => {});
}

function closeWindow(event) {
  event?.stopPropagation();
  closeTauriWindow({ force: true }).catch(() => {});
}

onMounted(() => {
  store.initialize();
  document.addEventListener('pointerdown', handleDocumentPointerDown, true);
  window.addEventListener(OPEN_REMOTE_STORAGE_EVENT, openRemoteModal);
  window.addEventListener(CREATE_SIDEBAR_GROUP_EVENT, createFavoriteGroup);
  window.addEventListener('keydown', handleWindowKeydown);
  window.addEventListener('resize', handleWindowResize);
  window.addEventListener('pointermove', handlePointerFileDragMove, true);
  window.addEventListener('pointerup', clearPointerFileDragIndicator, true);
  window.addEventListener('pointercancel', clearPointerFileDragIndicator, true);
  volumeRefreshTimer = window.setInterval(() => {
    store.refreshVolumes();
  }, 7000);
});

onUnmounted(() => {
  document.removeEventListener('pointerdown', handleDocumentPointerDown, true);
  window.removeEventListener(OPEN_REMOTE_STORAGE_EVENT, openRemoteModal);
  window.removeEventListener(CREATE_SIDEBAR_GROUP_EVENT, createFavoriteGroup);
  window.removeEventListener('keydown', handleWindowKeydown);
  window.removeEventListener('resize', handleWindowResize);
  window.removeEventListener('pointermove', handlePointerFileDragMove, true);
  window.removeEventListener('pointerup', clearPointerFileDragIndicator, true);
  window.removeEventListener('pointercancel', clearPointerFileDragIndicator, true);

  if (volumeRefreshTimer) {
    window.clearInterval(volumeRefreshTimer);
  }
});
</script>

<template>
  <aside
    class="sidebar"
    :class="{ 'sidebar--hidden': !store.sidebarVisible }"
    aria-label="Bookmarks"
  >
    <div
      class="sidebar-window-region"
      @mousedown="startDragging"
      @dblclick="toggleMaximizeWindow"
    >
      <div class="window-controls" aria-label="Window actions" @mousedown.stop @dblclick.stop>
        <button
          type="button"
          class="window-control window-control--close"
          aria-label="Close window"
          @pointerdown.stop
          @mousedown.stop
          @dblclick.stop
          @click.stop.prevent="closeWindow"
        >
          <span aria-hidden="true"></span>
        </button>
        <button
          type="button"
          class="window-control window-control--minimize"
          aria-label="Minimize window"
          @pointerdown.stop
          @mousedown.stop
          @dblclick.stop
          @click.stop.prevent="minimizeWindow"
        >
          <span aria-hidden="true"></span>
        </button>
        <button
          type="button"
          class="window-control window-control--zoom"
          aria-label="Zoom window"
          @pointerdown.stop
          @mousedown.stop
          @dblclick.stop
          @click.stop.prevent="toggleMaximizeWindow"
        >
          <span aria-hidden="true"></span>
        </button>
      </div>
    </div>

    <div
      class="sidebar-scroll"
      @dragenter="handleSidebarFileDragOver"
      @dragover="handleSidebarFileDragOver"
      @dragleave="handleSidebarFileDragLeave"
      @drop="handleSidebarFileDrop"
    >
      <div
        v-for="section in store.sidebarSections"
        :key="section.id || section.title"
        class="sidebar-section"
        :data-favorite-drop-zone="isFavoriteGroupSection(section) ? 'true' : null"
        :data-favorite-group-id="isFavoriteGroupSection(section) ? favoriteGroupIdForSection(section) : null"
        :class="{
          'sidebar-section--favorite-drop': isFavoriteDropTarget(section),
          'sidebar-section--favorite-drop-end': isFavoriteDropTargetEnd(section),
        }"
        @dragenter="handleFavoriteSectionDragOver(section, $event)"
        @dragover="handleFavoriteSectionDragOver(section, $event)"
        @dragleave="handleFavoriteSectionDragLeave(section, $event)"
        @drop="handleFavoriteSectionDrop(section, $event)"
      >
        <div class="sidebar-section-header">
          <h2>{{ section.title }}</h2>
          <button
            v-if="section.isFavoriteGroup && !section.isDefaultFavoriteGroup"
            type="button"
            class="sidebar-section-action"
            aria-label="Delete group"
            title="Delete group"
            @click="removeFavoriteGroup(section, $event)"
          >
            <AppIcon name="x" :size="13" :stroke-width="2.2" />
          </button>
          <button
            v-if="section.isRecentGroup"
            type="button"
            class="sidebar-section-action"
            aria-label="Clear recent locations"
            title="Clear recent"
            @click.stop="store.clearRecentLocations()"
          >
            <AppIcon name="trash" :size="13" :stroke-width="2" />
          </button>
        </div>
        <div
          v-if="isFavoriteGroupSection(section) && section.items.length === 0"
          class="sidebar-empty-group-drop"
          aria-hidden="true"
        ></div>
        <div
          v-for="item in section.items"
          :key="`${section.title}-${item.id || item.path || item.devicePath || item.name}`"
          class="sidebar-item-shell"
          :class="{
            'sidebar-item-shell--favorite-dragging': draggedFavoriteId === item.id,
            'sidebar-item-shell--favorite-drop-before': isFavoriteItemDropBefore(item),
          }"
          :data-favorite-index="item.isFavorite ? favoriteIndexForItem(item) : null"
          :data-favorite-id="item.isFavorite ? item.id : null"
          :data-favorite-group-id="item.isFavorite ? favoriteGroupIdForItem(item) : null"
          :draggable="item.isFavorite"
          @dragstart.stop="handleFavoriteDragStart(item, $event)"
          @dragend.stop="handleFavoriteDragEnd"
          @dragenter="handleFavoriteItemDragOver(item, $event)"
          @dragover="handleFavoriteItemDragOver(item, $event)"
          @drop="handleFavoriteItemDrop(item, $event)"
          @contextmenu.prevent.stop="openSidebarItemContextMenu(section, item, $event)"
        >
          <button
            type="button"
            class="sidebar-item"
            :class="{
              'sidebar-item--disabled': item.disabled,
              'sidebar-item--mounting': mountingDevicePath === item.devicePath,
              'sidebar-item--remote': item.isRemote,
              'sidebar-item--actionable': item.isRemote || item.isFavorite || item.canEject,
            }"
            :disabled="item.disabled || mountingDevicePath === item.devicePath"
            @click="openSidebarItem(item)"
            @keydown="handleSidebarItemKeydown(section, item, $event)"
          >
            <span class="sidebar-symbol" :style="{ '--item-color': item.color }" aria-hidden="true">
              <AppIcon :name="item.icon || 'folder'" :size="18" :stroke-width="1.9" />
            </span>
            <span class="sidebar-label">{{ item.name }}</span>
            <small v-if="item.detail || mountingDevicePath === item.devicePath">
              {{ mountingDevicePath === item.devicePath ? deviceBusyLabel(item) : item.detail }}
            </small>
          </button>
          <span v-if="item.isRemote" class="sidebar-item-actions">
            <button
              type="button"
              class="sidebar-item-action"
              aria-label="Check remote connection"
              title="Check remote connection"
              @click="refreshRemoteItem(item, $event)"
            >
              <AppIcon name="refresh" :size="13" :stroke-width="2.2" />
            </button>
            <button
              type="button"
              class="sidebar-item-action"
              aria-label="Disconnect remote volume"
              title="Disconnect remote volume"
              @click="disconnectRemoteItem(item, $event)"
            >
              <AppIcon name="x" :size="13" :stroke-width="2.2" />
            </button>
          </span>
          <button
            v-else-if="item.canEject"
            type="button"
            class="sidebar-item-action"
            aria-label="Eject device"
            title="Eject device"
            :disabled="mountingDevicePath === item.devicePath"
            @click="ejectSidebarDevice(item, $event)"
          >
            <AppIcon name="eject" :size="13" :stroke-width="2.2" />
          </button>
          <button
            v-else-if="item.isFavorite"
            type="button"
            class="sidebar-item-action"
            aria-label="Remove favorite"
            title="Remove favorite"
            @click="removeFavoriteItem(item, $event)"
          >
            <AppIcon name="x" :size="13" :stroke-width="2.2" />
          </button>
        </div>
      </div>
    </div>
    <footer ref="sidebarFooter" class="sidebar-footer">
      <button
        type="button"
        class="sidebar-footer-btn"
        aria-label="Add to sidebar"
        aria-haspopup="menu"
        :aria-expanded="sidebarAddMenuOpen"
        v-tooltip="{ text: 'Add to Sidebar', description: 'Add a remote storage or group' }"
        @click="toggleSidebarAddMenu"
      >
        <AppIcon name="plus" :size="16" :stroke-width="2.1" />
      </button>

      <WorkspaceSelector
        v-if="store.sidebarVisible"
        placement="above"
        variant="sidebar"
      />

      <Transition name="sidebar-add-menu">
        <div
          v-if="sidebarAddMenuOpen"
          class="sidebar-add-menu"
          role="menu"
          aria-label="Add to sidebar"
          @click.stop
        >
          <button type="button" role="menuitem" @click="openRemoteModal">
            <span class="sidebar-add-menu-icon" aria-hidden="true">
              <AppIcon name="network" :size="16" :stroke-width="1.9" />
            </span>
            <span>
              <strong>Remote Storage</strong>
              <small>SFTP, FTP, WebDAV, S3</small>
            </span>
          </button>
          <button type="button" role="menuitem" @click="createFavoriteGroup">
            <span class="sidebar-add-menu-icon" aria-hidden="true">
              <AppIcon name="folder-plus" :size="16" :stroke-width="1.9" />
            </span>
            <span>
              <strong>New Group</strong>
              <small>Organize sidebar shortcuts</small>
            </span>
          </button>
        </div>
      </Transition>
    </footer>
    <RemoteVolumeModal
      v-if="remoteModalVisible"
      :visible="remoteModalVisible"
      @close="closeRemoteModal"
    />

    <Teleport to="body">
      <div
        v-if="sidebarItemMenu"
        ref="sidebarItemMenuRef"
        class="sidebar-context-menu"
        :style="sidebarItemMenuStyle"
        role="menu"
        :aria-label="`${sidebarItemMenuTitle} sidebar item menu`"
        @click.stop
        @contextmenu.prevent
        @keydown.stop="handleSidebarItemContextKeydown"
      >
        <div class="sidebar-context-menu-title">
          <span>{{ sidebarItemMenuTitle }}</span>
          <small>{{ sidebarItemMenuDetail }}</small>
        </div>

        <template
          v-for="(group, groupIndex) in sidebarItemMenuGroups"
          :key="group.id"
        >
          <div v-if="groupIndex > 0" class="sidebar-context-menu-separator"></div>
          <div v-if="group.label" class="sidebar-context-menu-section-label">{{ group.label }}</div>

          <button
            v-for="menuItem in group.items"
            :key="menuItem.id"
            type="button"
            role="menuitem"
            class="sidebar-context-menu-item"
            :class="{ 'sidebar-context-menu-item--danger': menuItem.danger }"
            :disabled="menuItem.disabled"
            @click="runSidebarItemContextAction(menuItem)"
          >
            <AppIcon :name="menuItem.icon" :size="16" />
            <span>{{ menuItem.label }}</span>
          </button>
        </template>
      </div>
    </Teleport>
  </aside>
</template>

<style scoped>
.sidebar {
  display: flex;
  flex-direction: column;
  min-width: 0;
  overflow: hidden;
  border-radius: 0;
  background: var(--sidebar-bg);
  box-shadow:
    inset -1px 0 0 var(--separator),
    inset 0 1px 0 var(--hairline);
  transition: opacity 180ms ease;
}

.sidebar--hidden {
  opacity: 0;
  pointer-events: none;
}

.sidebar-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 0 14px 20px;
}

.sidebar-window-region {
  display: flex;
  min-height: 65px;
  align-items: center;
  flex: 0 0 auto;
}

/* ── Traffic lights ───────────────────────────────────────── */
.window-controls {
  display: flex;
  align-items: center;
  gap: 10px;
  padding-left: 12px;
}

.window-control {
  position: relative;
  display: grid;
  width: 13px;
  height: 13px;
  place-items: center;
  border-radius: 50%;
  padding: 0;
  box-shadow:
    inset 0 0 0 0.5px rgb(0 0 0 / 0.35),
    0 1px 2px rgb(0 0 0 / 0.25);
}

.window-control span {
  width: 6px;
  height: 6px;
  opacity: 0;
  transition: opacity 90ms ease;
}

.window-controls:hover .window-control span {
  opacity: 0.75;
}

.window-control--close { background: var(--traffic-close); }
.window-control--minimize { background: var(--traffic-minimize); }
.window-control--zoom { background: var(--traffic-zoom); }

.window-control--close span::before,
.window-control--close span::after {
  position: absolute;
  top: 6px;
  left: 3.7px;
  width: 5.7px;
  height: 1px;
  border-radius: 1px;
  background: rgb(80 0 0 / 0.75);
  content: "";
}

.window-control--close span::before { transform: rotate(45deg); }
.window-control--close span::after { transform: rotate(-45deg); }

.window-control--minimize span::before {
  position: absolute;
  top: 6px;
  left: 3.8px;
  width: 5.7px;
  height: 1.2px;
  border-radius: 1px;
  background: rgb(88 58 0 / 0.75);
  content: "";
}

.window-control--zoom span::before {
  position: absolute;
  top: 3.9px;
  left: 4px;
  width: 4.8px;
  height: 4.8px;
  border: 1px solid rgb(0 70 14 / 0.68);
  border-radius: 1px;
  content: "";
}

/* ── Sections ────────────────────────────────────────────── */
.sidebar-section + .sidebar-section {
  margin-top: 18px;
}

.sidebar-section--favorite-drop {
  border-radius: 8px;
}

.sidebar-section--favorite-drop-end::after {
  display: block;
  height: 2px;
  margin: 5px 8px 0;
  border-radius: 999px;
  background: var(--accent);
  box-shadow: 0 0 0 2px var(--accent-glow);
  content: "";
}

.sidebar-section-header {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  min-height: 25px;
}

.sidebar-section-action {
  display: grid;
  width: 22px;
  height: 22px;
  place-items: center;
  border-radius: 6px;
  padding: 0;
  background: transparent;
  color: var(--text-faint);
  opacity: 0;
  pointer-events: none;
  transition:
    background 100ms ease,
    color 100ms ease,
    opacity 100ms ease;
}

.sidebar-section:hover .sidebar-section-action,
.sidebar-section:focus-within .sidebar-section-action,
.sidebar-section-action:focus-visible {
  opacity: 1;
  pointer-events: auto;
}

.sidebar-section-action:hover,
.sidebar-section-action:focus-visible {
  background: var(--btn-hover);
  color: var(--text);
}

.sidebar-empty-group-drop {
  min-height: 32px;
  margin: 1px 0 0;
  border-radius: 7px;
  transition: background 100ms ease, box-shadow 100ms ease;
}

.sidebar-section--favorite-drop .sidebar-empty-group-drop {
  background: color-mix(in srgb, var(--accent) 10%, transparent);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 34%, transparent);
}

h2 {
  margin: 0 0 4px 6px;
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0;
}

/* ── Items ───────────────────────────────────────────────── */
.sidebar-item-shell {
  position: relative;
}

.sidebar-item-shell--favorite-dragging {
  opacity: 0.5;
}

.sidebar-item-shell--favorite-drop-before::before {
  position: absolute;
  z-index: 2;
  top: -1px;
  right: 8px;
  left: 8px;
  height: 2px;
  border-radius: 999px;
  background: var(--accent);
  box-shadow: 0 0 0 2px var(--accent-glow);
  content: "";
}

.sidebar-item {
  display: grid;
  grid-template-columns: 24px minmax(0, 1fr) auto;
  width: 100%;
  align-items: center;
  gap: 7px;
  min-height: 35px;
  border-radius: 7px;
  padding: 0 8px;
  background: transparent;
  color: var(--text);
  text-align: left;
  transition: background 100ms ease;
}

.sidebar-item:hover {
  background: var(--btn-hover);
}

.sidebar-item--disabled {
  cursor: default;
  opacity: 0.62;
}

.sidebar-item--disabled:hover {
  background: transparent;
}

.sidebar-item--mounting {
  cursor: wait;
  opacity: 0.72;
}

.sidebar-item--remote {
  padding-right: 58px;
}

.sidebar-item--actionable:not(.sidebar-item--remote) {
  padding-right: 34px;
}

.sidebar-symbol {
  display: grid;
  width: 22px;
  height: 22px;
  place-items: center;
  color: var(--item-color, var(--accent));
}

.sidebar-label {
  overflow: hidden;
  font-size: 13px;
  font-weight: 590;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sidebar-item small {
  overflow: hidden;
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 590;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sidebar-item-action {
  position: absolute;
  top: 50%;
  right: 6px;
  display: grid;
  width: 22px;
  height: 22px;
  place-items: center;
  padding: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--text-faint);
  opacity: 0;
  pointer-events: none;
  transform: translateY(-50%);
  transition:
    background 100ms ease,
    color 100ms ease,
    opacity 100ms ease;
}

.sidebar-item-actions {
  position: absolute;
  top: 50%;
  right: 6px;
  display: flex;
  align-items: center;
  gap: 2px;
  opacity: 0;
  pointer-events: none;
  transform: translateY(-50%);
  transition: opacity 100ms ease;
}

.sidebar-item-actions .sidebar-item-action {
  position: static;
  transform: none;
}

.sidebar-item-shell:hover .sidebar-item-action,
.sidebar-item-shell:hover .sidebar-item-actions,
.sidebar-item-shell:focus-within .sidebar-item-action,
.sidebar-item-shell:focus-within .sidebar-item-actions,
.sidebar-item-actions:focus-within,
.sidebar-item-action:focus-visible {
  opacity: 1;
  pointer-events: auto;
}

.sidebar-item-action:hover,
.sidebar-item-action:focus-visible {
  background: var(--btn-hover);
  color: var(--text);
}

.sidebar-item-action:disabled {
  cursor: wait;
  color: var(--text-faint);
}

/* ── Context menu ────────────────────────────────────────── */
.sidebar-context-menu {
  position: fixed;
  z-index: 2100;
  width: min(276px, calc(100vw - 16px));
  overflow-x: hidden;
  overflow-y: auto;
  overscroll-behavior: contain;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  padding: 5px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  animation: sidebar-context-appear 130ms cubic-bezier(0.2, 0, 0, 1) both;
  transform-origin: top left;
  scrollbar-width: thin;
  scrollbar-color: var(--control-border) transparent;
}

.sidebar-context-menu::-webkit-scrollbar {
  width: 9px;
}

.sidebar-context-menu::-webkit-scrollbar-track {
  background: transparent;
}

.sidebar-context-menu::-webkit-scrollbar-thumb {
  border: 2px solid transparent;
  border-radius: 999px;
  background: var(--control-border);
  background-clip: padding-box;
}

@keyframes sidebar-context-appear {
  from {
    opacity: 0;
    transform: scale(0.93);
  }

  to {
    opacity: 1;
    transform: scale(1);
  }
}

.sidebar-context-menu-title {
  display: grid;
  gap: 1px;
  padding: 5px 10px 8px;
  border-bottom: 1px solid var(--hairline);
  margin-bottom: 4px;
}

.sidebar-context-menu-title span,
.sidebar-context-menu-title small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sidebar-context-menu-title span {
  font-size: 12.5px;
  font-weight: 660;
}

.sidebar-context-menu-title small {
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 620;
}

.sidebar-context-menu-section-label {
  padding: 4px 10px 3px;
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: uppercase;
}

.sidebar-context-menu-item {
  display: grid;
  width: 100%;
  grid-template-columns: 18px minmax(0, 1fr);
  align-items: center;
  gap: 9px;
  min-height: 26px;
  border-radius: 8px;
  padding: 0 10px;
  background: transparent;
  color: var(--text);
  font-size: 13px;
  font-weight: 500;
  text-align: left;
  transition: none;
}

.sidebar-context-menu-item span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sidebar-context-menu-item:hover:not(:disabled),
.sidebar-context-menu-item:focus-visible {
  background: var(--btn-primary-bg);
  color: #fff;
  outline: 0;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.sidebar-context-menu-item:disabled {
  cursor: default;
  color: var(--text-faint);
  opacity: 0.45;
}

.sidebar-context-menu-item--danger {
  color: var(--danger);
}

.sidebar-context-menu-item--danger:hover:not(:disabled),
.sidebar-context-menu-item--danger:focus-visible {
  background: var(--btn-danger-bg);
  color: #fff;
}

.sidebar-context-menu-separator {
  height: 1px;
  margin: 4px 2px;
  background: var(--hairline);
}

/* ── Footer ──────────────────────────────────────────────── */
.sidebar-footer {
  position: relative;
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px 12px;
  border-top: 1px solid var(--separator);
}

.sidebar-footer-btn {
  display: grid;
  place-items: center;
  width: 30px;
  height: 30px;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.sidebar-footer-btn:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.sidebar-add-menu {
  position: absolute;
  z-index: 20;
  bottom: calc(100% + 8px);
  left: 10px;
  display: grid;
  width: min(238px, calc(100% - 20px));
  gap: 3px;
  border: 1px solid var(--control-border);
  border-radius: 9px;
  padding: 5px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
}

.sidebar-add-menu button {
  display: grid;
  grid-template-columns: 26px minmax(0, 1fr);
  align-items: center;
  gap: 8px;
  width: 100%;
  min-height: 43px;
  border-radius: 7px;
  padding: 6px 8px;
  background: transparent;
  color: var(--text);
  text-align: left;
  transition: background 100ms ease, color 100ms ease;
}

.sidebar-add-menu button:hover,
.sidebar-add-menu button:focus-visible {
  background: var(--btn-hover);
}

.sidebar-add-menu-icon {
  display: grid;
  width: 24px;
  height: 24px;
  place-items: center;
  color: var(--accent);
}

.sidebar-add-menu strong,
.sidebar-add-menu small {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sidebar-add-menu strong {
  font-size: 12.5px;
  font-weight: 650;
  letter-spacing: 0;
}

.sidebar-add-menu small {
  margin-top: 2px;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
}

.sidebar-add-menu-enter-active,
.sidebar-add-menu-leave-active {
  transition: opacity 90ms ease, transform 90ms ease;
}

.sidebar-add-menu-enter-from,
.sidebar-add-menu-leave-to {
  opacity: 0;
  transform: translateY(4px);
}
</style>
