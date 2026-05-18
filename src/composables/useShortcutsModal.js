import { ref } from 'vue';

const visible = ref(false);

export function useShortcutsModal() {
  return {
    visible,
    show: () => { visible.value = true; },
    hide: () => { visible.value = false; },
  };
}
