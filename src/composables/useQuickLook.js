import { computed, ref } from 'vue';

const visible = ref(false);
const entries = ref([]);
const index = ref(0);

const current = computed(() => entries.value[index.value] || null);
const count = computed(() => entries.value.length);

export function useQuickLook() {
  return {
    visible,
    entries,
    index,
    current,
    count,
    open(list, startIndex = 0) {
      const items = (Array.isArray(list) ? list : []).filter(Boolean);

      if (items.length === 0) {
        return;
      }

      entries.value = items;
      index.value = Math.min(Math.max(0, startIndex), items.length - 1);
      visible.value = true;
    },
    close() {
      visible.value = false;
      entries.value = [];
      index.value = 0;
    },
    next() {
      if (index.value < entries.value.length - 1) {
        index.value += 1;
      }
    },
    prev() {
      if (index.value > 0) {
        index.value -= 1;
      }
    },
  };
}
