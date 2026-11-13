import { FlaskConical, Plus, ScrollText, Trash } from 'lucide-react';
import { useState } from 'react';
import { useConnection } from '@/api/connection';
import { useEvent, useRpc, useRpcMutation } from '@/api/hooks';
import { Button } from '@/design/Button';
import { confirm, Dialog } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { SearchInput } from '@/design/SearchInput';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { fuzzyFilter } from '@/lib/fuzzy';
import type { DevSnippet } from '../../mocks/dev';
import { Example, Section } from '../parts';
import styles from '../Showcase.module.css';

const COLUMNS: TableColumn<DevSnippet>[] = [
  { id: 'title', header: 'Title', required: true, sortValue: (snippet) => snippet.title, cell: (snippet) => snippet.title },
  { id: 'command', header: 'Command', mono: true, sortValue: (snippet) => snippet.command, cell: (snippet) => snippet.command },
  {
    id: 'created',
    header: 'Created',
    width: 120,
    align: 'right',
    sortValue: (snippet) => snippet.created_at,
    cell: (snippet) => <TimeAgo at={snippet.created_at} />,
  },
];

/**
 * A complete, small screen built the way real ones are: read with `useRpc`
 * (refetched by backend events), write with `useRpcMutation`, and the three
 * states every screen handles: loading, empty and error.
 */
function Snippets() {
  const [query, setQuery] = useState('');
  const [adding, setAdding] = useState(false);
  const [title, setTitle] = useState('');
  const [command, setCommand] = useState('');
  const [lastEvent, setLastEvent] = useState<string | null>(null);

  const snippets = useRpc<DevSnippet[]>('dev.snippets.list', undefined, {
    invalidateOn: ['dev.snippets.*'],
  });
  const create = useRpcMutation<DevSnippet, { title: string; command: string }>('dev.snippets.create', {
    successToast: 'Snippet saved',
    // The dialog shows the error next to the field that caused it.
    errorToast: false,
  });
  const remove = useRpcMutation<null, { id: string }>('dev.snippets.delete', {
    successToast: 'Snippet deleted',
  });
  useEvent<{ change: string; id: string }>('dev.snippets.changed', (payload) => {
    setLastEvent(`${payload.change} ${payload.id}`);
  });

  const closeDialog = () => {
    setAdding(false);
    setTitle('');
    setCommand('');
    create.reset();
  };

  if (snippets.isError) {
    return <ErrorState size="compact" title="Snippets could not be loaded" error={snippets.error} onRetry={() => void snippets.refetch()} />;
  }

  const rows = fuzzyFilter(snippets.data ?? [], query, {
    text: (snippet) => snippet.title,
    keywords: (snippet) => [snippet.command],
  }).map((result) => result.item);

  return (
    <div className={styles.stack}>
      <Toolbar>
        <div style={{ width: 240 }}>
          <SearchInput aria-label="Search snippets" value={query} onChange={setQuery} />
        </div>
        <span className={styles.note}>{lastEvent ? `Last event: ${lastEvent}` : 'No events yet'}</span>
        <ToolbarSpacer />
        <Button variant="primary" icon={Plus} onClick={() => setAdding(true)}>
          Add snippet
        </Button>
      </Toolbar>

      <Table
        aria-label="Snippets"
        columns={COLUMNS}
        rows={rows}
        rowKey={(snippet) => snippet.id}
        loading={snippets.isPending}
        defaultSort={{ columnId: 'created', direction: 'desc' }}
        rowActions={(snippet) => [
          {
            label: 'Delete',
            icon: Trash,
            tone: 'danger',
            onSelect: async () => {
              const confirmed = await confirm({
                title: `Delete “${snippet.title}”?`,
                description: 'The snippet is removed for everyone. Commands already run are not affected.',
                confirmLabel: 'Delete',
                tone: 'danger',
              });
              if (confirmed) remove.mutate({ id: snippet.id });
            },
          },
        ]}
        empty={
          query ? (
            <EmptyState
              size="compact"
              icon={ScrollText}
              title="No snippet matches"
              description={`Nothing matches “${query}”.`}
              action={
                <Button size="sm" onClick={() => setQuery('')}>
                  Clear search
                </Button>
              }
            />
          ) : (
            <EmptyState
              size="compact"
              icon={ScrollText}
              title="No snippets yet"
              description="Save the commands you run most, and send them to any device in one step."
              action={
                <Button variant="primary" size="sm" icon={Plus} onClick={() => setAdding(true)}>
                  Add snippet
                </Button>
              }
            />
          )
        }
      />

      <Dialog
        open={adding}
        onClose={closeDialog}
        title="Add snippet"
        description="Leave a field empty to see the backend's error where it belongs."
        dismissible={!create.isPending}
        footer={
          <>
            <Button onClick={closeDialog} disabled={create.isPending}>
              Cancel
            </Button>
            <Button
              variant="primary"
              loading={create.isPending}
              onClick={() => create.mutate({ title, command }, { onSuccess: closeDialog })}
            >
              Save snippet
            </Button>
          </>
        }
      >
        <div className={styles.stack}>
          <Field label="Title" required>
            <Input data-autofocus value={title} onChange={(event) => setTitle(event.target.value)} />
          </Field>
          <Field label="Command" required error={create.error?.message}>
            <Input mono value={command} onChange={(event) => setCommand(event.target.value)} />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}

function FailingCall() {
  const failing = useRpc<never>('dev.fail', { code: 'timeout', message: 'edge-fw-01 did not answer within 10 seconds.' });
  if (failing.isPending) return <p className={styles.note}>Calling dev.fail…</p>;
  return <ErrorState size="compact" title="The routing table could not be read" error={failing.error} onRetry={() => void failing.refetch()} />;
}

export function Patterns() {
  const transport = useConnection((connection) => connection.transport);

  if (transport !== 'mock') {
    return (
      <EmptyState
        icon={FlaskConical}
        title="These examples need the mock transport"
        description="They call dev.* methods that exist only in the mock. Open the showcase with ?mock in the address, before the #."
      />
    );
  }

  return (
    <Section
      title="A data-backed screen"
      description="The conventions every screen follows, in one working example against the mock transport: a toolbar with filters on the left and the primary action on the right, then content with its loading, empty and error states."
    >
      <Example title="useRpc + invalidateOn, useRpcMutation, useEvent" span={2} layout="fill">
        <Snippets />
      </Example>
      <Example title="A failed call" layout="fill">
        <FailingCall />
      </Example>
    </Section>
  );
}
