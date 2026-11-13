// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useToasts } from '@/design/Toast';
import { registerCommand, runCommand, useCommands } from './commands';

const ids = () => Object.keys(useCommands.getState().commands);

beforeEach(() => {
  localStorage.clear();
  useCommands.setState({ commands: {}, recent: [] });
  useToasts.setState({ toasts: [], history: [], unread: 0 });
});

describe('registerCommand', () => {
  it('adds a command and removes it again', () => {
    const unregister = registerCommand({ id: 'devices.add', title: 'Add device', group: 'Devices', run: () => undefined });
    expect(ids()).toEqual(['devices.add']);
    unregister();
    expect(ids()).toEqual([]);
  });

  it('keeps a newer registration when an older one is removed', () => {
    const first = registerCommand({ id: 'x', title: 'First', group: 'G', run: () => undefined });
    registerCommand({ id: 'x', title: 'Second', group: 'G', run: () => undefined });
    first();
    expect(useCommands.getState().commands.x?.title).toBe('Second');
  });

  it('rejects a malformed shortcut where it was written', () => {
    expect(() => registerCommand({ id: 'x', title: 'X', group: 'G', shortcut: 'hyper+k', run: () => undefined })).toThrow(
      /unknown modifier/,
    );
    expect(ids()).toEqual([]);
  });
});

describe('runCommand', () => {
  it('runs the command and remembers it, most recent first, without duplicates', () => {
    const runs: string[] = [];
    for (const id of ['a', 'b', 'c']) {
      registerCommand({ id, title: id, group: 'G', run: () => void runs.push(id) });
    }
    runCommand('a');
    runCommand('b');
    runCommand('a');

    expect(runs).toEqual(['a', 'b', 'a']);
    expect(useCommands.getState().recent).toEqual(['a', 'b']);
    expect(JSON.parse(localStorage.getItem('nocturne.palette.recent') ?? '[]')).toEqual(['a', 'b']);
  });

  it('keeps only the latest few', () => {
    for (let index = 0; index < 10; index++) {
      registerCommand({ id: `c${index}`, title: `c${index}`, group: 'G', run: () => undefined });
      runCommand(`c${index}`);
    }
    expect(useCommands.getState().recent).toEqual(['c9', 'c8', 'c7', 'c6', 'c5', 'c4']);
  });

  it('does nothing for an unknown id', () => {
    runCommand('nope');
    expect(useCommands.getState().recent).toEqual([]);
  });

  it('reports a command that throws, or rejects, instead of swallowing it', async () => {
    registerCommand({
      id: 'sync',
      title: 'Export report',
      group: 'G',
      run: () => {
        throw new Error('disk is full');
      },
    });
    registerCommand({ id: 'async', title: 'Start discovery', group: 'G', run: () => Promise.reject(new Error('no route to host')) });

    runCommand('sync');
    runCommand('async');

    await vi.waitFor(() => expect(useToasts.getState().toasts).toHaveLength(2));
    expect(useToasts.getState().toasts.map((toast) => [toast.tone, toast.title, toast.description])).toEqual([
      ['error', '"Export report" did not complete', 'disk is full'],
      ['error', '"Start discovery" did not complete', 'no route to host'],
    ]);
  });
});
