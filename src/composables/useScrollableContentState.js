import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue';

export function useScrollableContentState(targetRef, options = {}) {
  const isScrollable = ref(false);
  const threshold = Number(options.threshold ?? 1);
  let resizeObserver = null;
  let mutationObserver = null;
  let frame = 0;
  let timeout = 0;

  function cancelScheduledMeasure() {
    if (frame && typeof window !== 'undefined') {
      window.cancelAnimationFrame(frame);
    }

    if (timeout && typeof window !== 'undefined') {
      window.clearTimeout(timeout);
    }

    frame = 0;
    timeout = 0;
  }

  function measure() {
    frame = 0;
    timeout = 0;

    const target = targetRef.value;
    isScrollable.value = Boolean(
      target && target.scrollHeight - target.clientHeight > threshold,
    );
  }

  function scheduleMeasure() {
    cancelScheduledMeasure();

    if (typeof window !== 'undefined' && typeof window.requestAnimationFrame === 'function') {
      frame = window.requestAnimationFrame(measure);
      return;
    }

    timeout = setTimeout(measure, 0);
  }

  function disconnectObservers() {
    resizeObserver?.disconnect?.();
    mutationObserver?.disconnect?.();
    resizeObserver = null;
    mutationObserver = null;
  }

  function observeTarget() {
    disconnectObservers();
    const target = targetRef.value;
    scheduleMeasure();

    if (!target || typeof window === 'undefined') {
      return;
    }

    if (typeof ResizeObserver !== 'undefined') {
      resizeObserver = new ResizeObserver(scheduleMeasure);
      resizeObserver.observe(target);
    }

    if (typeof MutationObserver !== 'undefined') {
      mutationObserver = new MutationObserver(scheduleMeasure);
      mutationObserver.observe(target, {
        attributes: true,
        childList: true,
        characterData: true,
        subtree: true,
      });
    }
  }

  onMounted(() => {
    nextTick(observeTarget);
    if (typeof window !== 'undefined') {
      window.addEventListener('resize', scheduleMeasure);
    }
  });

  watch(targetRef, () => nextTick(observeTarget), { flush: 'post' });

  if (options.watch) {
    watch(options.watch, () => nextTick(scheduleMeasure), { flush: 'post' });
  }

  onUnmounted(() => {
    cancelScheduledMeasure();
    disconnectObservers();
    if (typeof window !== 'undefined') {
      window.removeEventListener('resize', scheduleMeasure);
    }
  });

  return {
    isScrollable,
    refreshScrollableState: scheduleMeasure,
  };
}
