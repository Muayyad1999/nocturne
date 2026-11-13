import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';

/**
 * Renders floating layers at the end of `<body>`, outside the app root, so
 * they stack in the order they open and stay usable while the app is inert.
 */
export function Portal({ children }: { children: ReactNode }) {
  return createPortal(children, document.body);
}
