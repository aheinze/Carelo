import { computed } from 'vue';
import { useFileManagerStore } from '../stores/fileManagerStore';

export function usePane(paneId) {
  const store = useFileManagerStore();
  const pane = computed(() => store.panes[paneId]);
  const activeTab = computed(() => store.activeTabFor(paneId));
  const entries = computed(() => store.visibleEntriesFor(paneId));

  return {
    pane,
    activeTab,
    entries,
    isActive: computed(() => store.activePaneId === paneId),
    load: () => store.loadPane(paneId),
    select: (index) => store.selectEntry(paneId, index),
  };
}
