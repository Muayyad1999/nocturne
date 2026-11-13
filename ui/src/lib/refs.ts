import { useCallback, type Ref, type RefCallback } from 'react';

function assign<T>(ref: Ref<T> | undefined, node: T | null): (() => void) | undefined {
  if (!ref) return undefined;
  if (typeof ref === 'function') {
    const cleanup = ref(node);
    return typeof cleanup === 'function' ? cleanup : () => ref(null);
  }
  ref.current = node;
  return () => {
    ref.current = null;
  };
}

/**
 * Combines several refs (object or callback) into one stable callback ref, so
 * a component can keep its own ref and still forward the caller's.
 */
export function useMergedRefs<T>(...refs: (Ref<T> | undefined)[]): RefCallback<T> {
  return useCallback(
    (node: T | null) => {
      const cleanups = refs.map((ref) => assign(ref, node));
      return () => {
        for (const cleanup of cleanups) cleanup?.();
      };
    },
    // The list of refs is the dependency list.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    refs,
  );
}
