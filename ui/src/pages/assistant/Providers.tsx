import { Cpu, Pencil, Plus, Star, Trash, Wrench } from 'lucide-react';
import { useEffect, useState } from 'react';
import { rpc } from '@/api/client';
import type { RpcError } from '@/api/errors';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  AiDeleted,
  LocalEndpoint,
  ProviderEntry,
  ProviderPreset,
  ProviderTestResult,
} from '@/api/types/ai';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Panel } from '@/design/Panel';
import { Skeleton } from '@/design/Skeleton';
import { StatusDot } from '@/design/StatusDot';
import { toast } from '@/design/Toast';
import { ProviderDrawer, TestResult } from './ProviderDrawer';
import styles from './Assistant.module.css';

export interface ProvidersProps {
  /**
   * The setup card sent the user here with a door in mind: a preset id, and
   * the address of a local runtime when that is what they chose.
   */
  opening: { presetId: string | null; baseUrl?: string } | null;
  onOpened: () => void;
}

/**
 * The models this installation can use: add one, test it, make it the
 * default, remove it.
 *
 * Everything a key touches is write-only — `ProviderEntry` carries `has_key`
 * and nothing else — so a saved key can only be replaced or cleared, never
 * read back, not even by this screen.
 */
export function Providers({ opening, onOpened }: ProvidersProps) {
  const providers = useRpc<ProviderEntry[]>('ai.providers.list', undefined, {
    invalidateOn: ['ai.providers.changed'],
  });
  const presets = useRpc<ProviderPreset[]>('ai.providers.presets');
  const local = useRpc<LocalEndpoint[]>('ai.providers.detect_local');
  const [editing, setEditing] = useState<ProviderEntry | null>(null);
  const [adding, setAdding] = useState<{
    preset: ProviderPreset | null;
    local?: LocalEndpoint | undefined;
  } | null>(null);
  const [results, setResults] = useState<Record<string, ProviderTestResult>>({});
  const [testing, setTesting] = useState<string | null>(null);

  // The setup card chose a door; open the editor on it, once the lists that
  // describe it have answered.
  const presetList = presets.data;
  const localList = local.data;
  useEffect(() => {
    if (!opening) return;
    if (opening.presetId && !presetList) return;
    if (opening.baseUrl && !localList) return;
    setAdding({
      preset: presetList?.find((item) => item.id === opening.presetId) ?? null,
      ...(opening.baseUrl
        ? { local: localList?.find((item) => item.base_url === opening.baseUrl) }
        : {}),
    });
    setEditing(null);
    onOpened();
  }, [opening, onOpened, presetList, localList]);

  const remove = useRpcMutation<AiDeleted, { id: string }>('ai.providers.delete', {
    invalidates: ['ai.'],
    errorToast: 'The model was not removed',
  });
  const save = useRpcMutation<ProviderEntry, { id: string; is_default: boolean }>(
    'ai.providers.save',
    { invalidates: ['ai.'], errorToast: 'That did not work' },
  );

  const test = (entry: ProviderEntry) => {
    setTesting(entry.id);
    void rpc<ProviderTestResult>('ai.providers.test', { id: entry.id })
      .then((result) => setResults((current) => ({ ...current, [entry.id]: result })))
      .catch((failure: RpcError) =>
        toast.error('The model could not be tested', { description: failure.message }),
      )
      .finally(() => setTesting(null));
  };

  const drop = async (entry: ProviderEntry) => {
    const sure = await confirm({
      title: `Remove ${entry.name}?`,
      description:
        'Its API key is deleted from the vault. Chats that used it stay, and will need another model to carry on.',
      confirmLabel: 'Remove',
      tone: 'danger',
    });
    if (sure) remove.mutate({ id: entry.id });
  };

  if (providers.isPending) {
    return (
      <div className={styles.loading} aria-busy="true" aria-label="Loading models">
        <Skeleton width="30%" />
        <Skeleton height={72} shape="block" />
        <Skeleton height={72} shape="block" />
      </div>
    );
  }
  if (providers.error) {
    return (
      <ErrorState
        title="The models could not be read"
        error={providers.error}
        onRetry={() => void providers.refetch()}
      />
    );
  }

  const found = local.data ?? [];
  const configured = providers.data ?? [];

  return (
    <div className={styles.providers}>
      {configured.length === 0 ? (
        <EmptyState
          icon={Cpu}
          title="No model is connected yet"
          description="Add Claude, any OpenAI-compatible endpoint, or a model running on this machine. The assistant can do nothing until one is here; nothing else in Nocturne needs it."
          action={
            <Button variant="primary" icon={Plus} onClick={() => setAdding({ preset: null })}>
              Add a model
            </Button>
          }
        />
      ) : (
        <Panel title="Models" padding="none">
          <ul className={styles.providerList}>
            {configured.map((entry) => (
              <li key={entry.id} className={styles.provider}>
                <div className={styles.providerMain}>
                  <p className={styles.providerName}>
                    {entry.name}
                    {entry.is_default && <Badge tone="ai">default</Badge>}
                    {!entry.enabled && <Badge tone="idle">off</Badge>}
                  </p>
                  <p className={styles.providerMeta}>
                    <span className="mono">{entry.model}</span>
                    <span className="mono">{entry.base_url}</span>
                  </p>
                  <p className={styles.providerMeta}>
                    <span className={styles.secretStatus}>
                      <StatusDot
                        tone={entry.has_key ? 'ok' : 'idle'}
                        label={entry.has_key ? 'API key set' : 'No API key'}
                      />
                      {entry.has_key ? 'API key set' : 'No API key'}
                    </span>
                    <span>tool mode: {entry.tool_mode}</span>
                    {entry.context_window ? <span>{entry.context_window} tokens</span> : null}
                  </p>
                  {entry.id in results && <TestResult result={results[entry.id] as ProviderTestResult} />}
                </div>
                <div className={styles.providerActions}>
                  <Button
                    size="sm"
                    icon={Wrench}
                    loading={testing === entry.id}
                    onClick={() => test(entry)}
                  >
                    Test
                  </Button>
                  {!entry.is_default && (
                    <Button
                      size="sm"
                      variant="ghost"
                      icon={Star}
                      onClick={() => save.mutate({ id: entry.id, is_default: true })}
                    >
                      Make default
                    </Button>
                  )}
                  <Button size="sm" variant="ghost" icon={Pencil} onClick={() => setEditing(entry)}>
                    Edit
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={Trash}
                    onClick={() => void drop(entry)}
                  >
                    Remove
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        </Panel>
      )}

      {found.length > 0 && (
        <Panel title="Running on this machine">
          <div className={styles.doors}>
            {found.map((endpoint) => {
              const preset = presets.data?.find((item) => item.id === endpoint.preset) ?? null;
              return (
                <div key={endpoint.base_url} className={styles.door} data-found>
                  <Icon icon={Cpu} size={14} />
                  <div className={styles.doorText}>
                    <strong>{preset?.name ?? endpoint.preset}</strong>
                    <span>
                      {endpoint.base_url} · {endpoint.models.length}{' '}
                      {endpoint.models.length === 1 ? 'model' : 'models'}. Nothing leaves your
                      network.
                    </span>
                  </div>
                  <Button size="sm" onClick={() => setAdding({ preset, local: endpoint })}>
                    Add it
                  </Button>
                </div>
              );
            })}
          </div>
        </Panel>
      )}

      <Panel
        title="Add from a preset"
        collapsible
        defaultCollapsed={configured.length > 0}
        storageId="assistant-presets"
      >
        <div className={styles.presetGrid}>
          {(presets.data ?? []).map((preset) => (
            <Card key={preset.id} className={styles.presetCard} padding="sm">
              <p className={styles.presetName}>
                {preset.name}
                {preset.local && <Badge tone="maint">local</Badge>}
                {preset.needs_key && <Badge tone="idle">needs a key</Badge>}
              </p>
              {preset.note && <p className={styles.presetNote}>{preset.note}</p>}
              <Button size="sm" icon={Plus} onClick={() => setAdding({ preset })}>
                Add
              </Button>
            </Card>
          ))}
        </div>
      </Panel>

      <ProviderDrawer
        open={Boolean(adding || editing)}
        onClose={() => {
          setAdding(null);
          setEditing(null);
        }}
        provider={editing}
        preset={adding?.preset ?? null}
        local={adding?.local ?? null}
      />
    </div>
  );
}
