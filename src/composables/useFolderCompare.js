import { ref } from 'vue';

const visible = ref(false);
const leftRoot = ref('');
const rightRoot = ref('');

export function useFolderCompare() {
  return {
    visible,
    leftRoot,
    rightRoot,
    open: (left, right) => {
      const leftPath = String(left || '').trim();
      const rightPath = String(right || '').trim();

      if (!leftPath || !rightPath) {
        return;
      }

      leftRoot.value = leftPath;
      rightRoot.value = rightPath;
      visible.value = true;
    },
    close: () => {
      visible.value = false;
    },
  };
}
