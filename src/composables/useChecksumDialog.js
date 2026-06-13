import { ref } from 'vue';

const visible = ref(false);
const paths = ref([]);

export function useChecksumDialog() {
  return {
    visible,
    paths,
    open: (targetPaths = []) => {
      const list = (Array.isArray(targetPaths) ? targetPaths : [targetPaths])
        .map((path) => String(path || '').trim())
        .filter(Boolean);

      if (list.length === 0) {
        return;
      }

      paths.value = list;
      visible.value = true;
    },
    close: () => {
      visible.value = false;
      paths.value = [];
    },
  };
}
