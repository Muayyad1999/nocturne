/**
 * The production bundle carries no development code.
 *
 * The mock transport is an in-memory backend: a build that shipped it could
 * answer a NOC engineer with invented data. It is reached through a dynamic
 * import behind `import.meta.env.DEV`, so the bundler drops it — this spec is
 * what proves the guard still holds. The component showcase goes the same way.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test } from '@playwright/test';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

const BUILD_TIMEOUT_MS = 180_000;

/** Takes the marker out of the source, so renaming it cannot quietly disable this check. */
async function mockMarker(): Promise<string> {
  const source = await readFile(join(UI_DIR, 'src/dev/mock.ts'), 'utf8');
  const found = /export const MOCK_MARKER = '([^']+)'/.exec(source);
  expect(found?.[1], 'src/dev/mock.ts must export MOCK_MARKER').toBeTruthy();
  return found![1]!;
}

async function filesIn(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { recursive: true, withFileTypes: true });
  return entries
    .filter((entry) => entry.isFile())
    .map((entry) => join(entry.parentPath, entry.name));
}

test.describe('the production bundle', () => {
  test.describe.configure({ timeout: BUILD_TIMEOUT_MS + 30_000 });

  test('does not contain the dev mock or the showcase', async () => {
    const marker = await mockMarker();
    const outDir = await mkdtemp(join(tmpdir(), 'nocturne-bundle-'));

    try {
      await run('pnpm', ['exec', 'vite', 'build', '--outDir', outDir, '--emptyOutDir'], {
        cwd: UI_DIR,
        timeout: BUILD_TIMEOUT_MS,
        maxBuffer: 16 * 1024 * 1024,
      });

      const files = await filesIn(outDir);
      expect(files.some((file) => file.endsWith('index.html'))).toBe(true);

      const text = files.filter((file) => /\.(js|css|html|map)$/.test(file));
      expect(text.length, 'the build produced no code').toBeGreaterThan(0);

      for (const file of text) {
        const content = await readFile(file, 'utf8');
        const name = file.slice(outDir.length + 1);
        expect(content, `${name} contains the mock transport`).not.toContain(marker);
        expect(content, `${name} exposes the mock controls`).not.toContain('__NOCTURNE_MOCK__');
        expect(content, `${name} contains the component showcase`).not.toContain('Component groups');
      }
    } finally {
      await rm(outDir, { recursive: true, force: true });
    }
  });
});
