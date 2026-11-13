import { defineConfig, devices } from '@playwright/test';

const port = Number(process.env.NOCTURNE_UI_PORT ?? 5273);
const baseURL = `http://127.0.0.1:${port}`;

/**
 * Two projects share one dev server:
 *   e2e    `pnpm e2e`    specs in tests/*.spec.ts
 *   shots  `pnpm shots`  tests/*.shots.ts, writes PNGs to tests/shots/ for visual review
 *
 * Screens are tested against the dev build with the mock transport (`?mock`),
 * so no backend is needed. The production bundle is checked by bundle.spec.ts.
 */
export default defineConfig({
  testDir: 'tests',
  outputDir: 'test-results',
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: [['list']],
  use: {
    baseURL,
    trace: 'off',
    reducedMotion: 'reduce',
  },
  webServer: {
    command: 'pnpm exec vite',
    url: baseURL,
    reuseExistingServer: !process.env.CI,
    stdout: 'ignore',
    stderr: 'pipe',
    timeout: 60_000,
  },
  projects: [
    {
      name: 'e2e',
      testMatch: /.*\.spec\.ts$/,
      use: { ...devices['Desktop Chrome'], viewport: { width: 1280, height: 800 } },
    },
    {
      name: 'shots',
      testMatch: /.*\.shots\.ts$/,
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1440, height: 900 },
        deviceScaleFactor: 2,
      },
    },
  ],
});
