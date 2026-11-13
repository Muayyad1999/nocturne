/**
 * `dev.*` methods back the showcase's data examples. They exist only in the
 * mock and are not part of the backend's surface.
 */

import { RpcError } from '@/api/errors';
import type { MockModule } from '../mock';

/** A bookmarked command: the kind of small record a real screen lists. */
export interface DevSnippet {
  id: string;
  title: string;
  command: string;
  created_at: number;
}

const snippets: DevSnippet[] = [
  { id: 's1', title: 'Interface summary', command: 'show ip interface brief', created_at: Date.now() - 86_400_000 },
  { id: 's2', title: 'BGP neighbours', command: 'show bgp summary', created_at: Date.now() - 3_600_000 },
];
let nextId = snippets.length + 1;

function record(params: unknown): Record<string, unknown> {
  return typeof params === 'object' && params !== null ? (params as Record<string, unknown>) : {};
}

const mocks: MockModule = {
  'dev.echo': (params) => params ?? null,

  'dev.fail': (params) => {
    const { code, message } = record(params);
    throw new RpcError(
      typeof code === 'string' ? code : 'unavailable',
      typeof message === 'string' ? message : 'The device did not answer within 10 seconds.',
    );
  },

  'dev.snippets.list': () => snippets,

  'dev.snippets.create': (params, { emit }) => {
    const { title, command } = record(params);
    if (typeof title !== 'string' || !title.trim() || typeof command !== 'string' || !command.trim()) {
      throw new RpcError('invalid', 'A snippet needs a title and a command.');
    }
    const snippet: DevSnippet = {
      id: `s${nextId++}`,
      title: title.trim(),
      command: command.trim(),
      created_at: Date.now(),
    };
    snippets.push(snippet);
    emit('dev.snippets.changed', { change: 'created', id: snippet.id });
    return snippet;
  },

  'dev.snippets.delete': (params, { emit }) => {
    const { id } = record(params);
    const index = snippets.findIndex((snippet) => snippet.id === id);
    if (index < 0) throw new RpcError('not_found', `snippet ${String(id)} was not found`);
    snippets.splice(index, 1);
    emit('dev.snippets.changed', { change: 'deleted', id });
    return null;
  },
};

export default mocks;
