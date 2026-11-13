/**
 * The route table. A hash router, so the same build works under tauri://,
 * file:// and http. Every screen is loaded on demand and has its own error
 * boundary: one broken screen never takes the shell down.
 *
 * To add a screen: create `src/pages/<area>/index.tsx` with a default export
 * and add one `screen(...)` line here (and a destination in destinations.ts
 * if it belongs in the nav rail).
 */

import { lazy, Suspense, type ComponentType } from 'react';
import { createHashRouter, type RouteObject } from 'react-router';
import { AppError } from './AppError';
import { AppShell } from './AppShell';
import { PageFallback } from './PageFallback';
import { RouteError } from './RouteError';

type ScreenModule = Promise<{ default: ComponentType }>;

function element(load: () => ScreenModule) {
  const Screen = lazy(load);
  return (
    <Suspense fallback={<PageFallback />}>
      <Screen />
    </Suspense>
  );
}

function screen(path: string, load: () => ScreenModule): RouteObject {
  return { path, element: element(load), ErrorBoundary: RouteError };
}

export function createAppRouter() {
  return createHashRouter([
    {
      path: '/',
      Component: AppShell,
      ErrorBoundary: AppError,
      children: [
        { index: true, element: element(() => import('@/pages/overview')), ErrorBoundary: RouteError },
        screen('devices/:id?', () => import('@/pages/devices')),
        screen('terminal', () => import('@/pages/terminal')),
        screen('topology', () => import('@/pages/topology')),
        screen('monitoring/:tab?', () => import('@/pages/monitoring')),
        screen('config/:tab?', () => import('@/pages/config')),
        screen('changes/:id?', () => import('@/pages/changes')),
        screen('automation/:tab?', () => import('@/pages/automation')),
        screen('lab/:id?', () => import('@/pages/lab')),
        screen('ipam', () => import('@/pages/ipam')),
        screen('tools/:tool?', () => import('@/pages/tools')),
        screen('ops/:tab?', () => import('@/pages/ops')),
        screen('assistant', () => import('@/pages/assistant')),
        screen('settings/:section?', () => import('@/pages/settings')),
        // The component showcase exists in development and test builds only.
        ...(import.meta.env.DEV ? [screen('dev/components/:group?', () => import('@/dev/showcase'))] : []),
        screen('*', () => import('@/pages/not-found')),
      ],
    },
  ]);
}

export type AppRouter = ReturnType<typeof createAppRouter>;
