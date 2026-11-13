/// <reference types="vitest/config" />
import { readFileSync } from 'node:fs';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')) as {
  version: string;
};

/** Dev server port. Non-default so several checkouts can run side by side. */
const port = Number(process.env.NOCTURNE_UI_PORT ?? 5273);

/**
 * Optional backend for browser development, e.g.
 * `NOCTURNE_API_URL=http://127.0.0.1:8787 pnpm dev`. Without it, use `?mock`.
 */
const apiUrl = process.env.NOCTURNE_API_URL;

export default defineConfig({
  // Relative asset URLs: the same build is served from tauri://, file:// and http.
  base: './',
  cacheDir: '.vite',
  plugins: [react()],
  resolve: {
    tsconfigPaths: true,
  },
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  css: {
    modules: {
      localsConvention: 'camelCaseOnly',
    },
  },
  server: {
    host: '127.0.0.1',
    port,
    strictPort: true,
    proxy: apiUrl ? { '/api': { target: apiUrl, changeOrigin: true, ws: true } } : undefined,
  },
  preview: {
    host: '127.0.0.1',
    port: port + 1,
    strictPort: true,
  },
  build: {
    // WebView2 (evergreen), WKWebView 16.4+ and current browsers for server mode.
    target: ['chrome111', 'safari16.4', 'firefox115'],
    sourcemap: false,
  },
  test: {
    // Pure logic runs in Node; tests that need a DOM start with
    // `// @vitest-environment happy-dom`.
    environment: 'node',
    include: ['src/**/*.test.{ts,tsx}'],
    restoreMocks: true,
    unstubGlobals: true,
  },
});
