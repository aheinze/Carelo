import { ref } from 'vue';

const visible = ref(false);
const target = ref(null);

export function usePermissionsDialog() {
  return {
    visible,
    target,
    open: (entry) => {
      const path = String(entry?.path || '').trim();

      if (!path) {
        return;
      }

      target.value = {
        path,
        name:
          entry?.name
          || path.replace(/\/+$/, '').split('/').filter(Boolean).at(-1)
          || path,
        isDirectory: entry?.kind === 'directory',
        isRemote: path.startsWith('remote://'),
      };
      visible.value = true;
    },
    close: () => {
      visible.value = false;
      target.value = null;
    },
  };
}
