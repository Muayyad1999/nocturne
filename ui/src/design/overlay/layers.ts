/**
 * The stack of open floating layers (dialogs, drawers, popovers, menus).
 *
 * One pair of document listeners serves every layer, so behaviour is the same
 * everywhere: Escape closes the top layer only; a press outside dismisses the
 * layers it falls outside of, top down, and stops at the first layer that
 * contains it or at a modal one. While a modal layer is open, everything below
 * it, including the app itself, is inert.
 */

import { useEffect, useEffectEvent, type RefObject } from 'react';

interface Layer {
  element: HTMLElement;
  modal: boolean;
  onEscape?: () => void;
  onPressOutside?: () => void;
  /** Elements outside `element` that still count as inside (the trigger). */
  contains?: (target: Node) => boolean;
}

const stack: Layer[] = [];

function isInside(layer: Layer, target: Node): boolean {
  return layer.element.contains(target) || Boolean(layer.contains?.(target));
}

function onKeyDown(event: KeyboardEvent): void {
  if (event.key !== 'Escape' || event.defaultPrevented) return;
  const top = stack[stack.length - 1];
  if (!top?.onEscape) return;
  event.preventDefault();
  top.onEscape();
}

function onPointerDown(event: PointerEvent): void {
  const target = event.target;
  if (!(target instanceof Node)) return;
  for (const layer of [...stack].reverse()) {
    if (isInside(layer, target)) return;
    layer.onPressOutside?.();
    if (layer.modal) return;
  }
}

/** Everything below the top modal layer is inert; nothing is when no modal is open. */
function syncInert(): void {
  const topModal = stack.findLastIndex((layer) => layer.modal);
  const root = document.getElementById('root');
  if (root) root.inert = topModal >= 0;
  stack.forEach((layer, index) => {
    layer.element.inert = index < topModal;
  });
}

function push(layer: Layer): () => void {
  if (stack.length === 0) {
    document.addEventListener('keydown', onKeyDown);
    document.addEventListener('pointerdown', onPointerDown, true);
  }
  stack.push(layer);
  syncInert();

  return () => {
    const index = stack.indexOf(layer);
    if (index >= 0) stack.splice(index, 1);
    layer.element.inert = false;
    syncInert();
    if (stack.length === 0) {
      document.removeEventListener('keydown', onKeyDown);
      document.removeEventListener('pointerdown', onPointerDown, true);
    }
  };
}

export interface LayerOptions {
  /** Blocks interaction with everything below while open. */
  modal?: boolean;
  onEscape?: () => void;
  onPressOutside?: () => void;
  /** An element outside the layer that counts as part of it, typically the trigger. */
  trigger?: RefObject<Element | null>;
}

/** Registers `ref` as an open layer for as long as `open` is true. */
export function useLayer(
  open: boolean,
  ref: RefObject<HTMLElement | null>,
  { modal = false, onEscape, onPressOutside, trigger }: LayerOptions,
): void {
  const escape = useEffectEvent(() => onEscape?.());
  const pressOutside = useEffectEvent(() => onPressOutside?.());

  useEffect(() => {
    const element = ref.current;
    if (!open || !element) return;
    return push({
      element,
      modal,
      onEscape: escape,
      onPressOutside: pressOutside,
      contains: (target) => Boolean(trigger?.current?.contains(target)),
    });
  }, [open, ref, modal, trigger]);
}
