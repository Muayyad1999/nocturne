import { CircleCheck, OctagonExclamationPoint, RefreshCw, Wrench } from 'lucide-react';
import { useEffect, useState } from 'react';
import { rpc } from '@/api/client';
import type { RpcError } from '@/api/errors';
import { useRpcMutation } from '@/api/hooks';
import type {
  AiProvidersSaveParams,
  Effort,
  LocalEndpoint,
  ModelInfo,
  ProviderEntry,
  ProviderKind,
  ProviderPreset,
  ProviderTestResult,
  ToolMode,
} from '@/api/types/ai';
import { Button } from '@/design/Button';
import { Combobox } from '@/design/Combobox';
import { Drawer } from '@/design/Drawer';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Spinner } from '@/design/Spinner';
import { Switch } from '@/design/Switch';
import { toast } from '@/design/Toast';
import { formatCount } from '@/lib/format';
import { SecretField } from './SecretField';
import { TOOL_CALLING } from './providerCopy';
import styles from './Assistant.module.css';

const KINDS: { value: ProviderKind; label: string }[] = [
  { value: 'anthropic', label: 'Anthropic (Claude)' },
  { value: 'openai', label: 'OpenAI-compatible' },
  { value: 'ollama', label: 'Ollama' },
];

const EFFORTS: { value: string; label: string }[] = [
  { value: '', label: 'The model’s own default' },
  { value: 'low', label: 'Low — quick answers' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High — think before answering' },
  { value: 'xhigh', label: 'Very high' },
  { value: 'max', label: 'Maximum' },
];

const TOOL_MODES: { value: ToolMode; label: string }[] = [
  { value: 'auto', label: 'Automatic — use the tool API, fall back to the prompt' },
  { value: 'native', label: 'Native — the model’s own tool API' },
  { value: 'prompted', label: 'Prompted — describe the tools in the prompt' },
];

export interface ProviderDrawerProps {
  open: boolean;
  onClose: () => void;
  /** The provider being edited, or null when adding one. */
  provider: ProviderEntry | null;
  /** Prefill for a new one. */
  preset?: ProviderPreset | null;
  local?: LocalEndpoint | null;
}

interface Draft {
  name: string;
  kind: ProviderKind;
  base_url: string;
  model: string;
  effort: string;
  tool_mode: ToolMode;
  context_window: string;
  is_default: boolean;
  enabled: boolean;
}

function draftOf(
  provider: ProviderEntry | null,
  preset: ProviderPreset | null | undefined,
  local: LocalEndpoint | null | undefined,
): Draft {
  if (provider) {
    return {
      name: provider.name,
      kind: provider.kind,
      base_url: provider.base_url,
      model: provider.model,
      effort: provider.effort ?? '',
      tool_mode: provider.tool_mode,
      context_window: provider.context_window ? String(provider.context_window) : '',
      is_default: provider.is_default,
      enabled: provider.enabled,
    };
  }
  return {
    name: preset?.name ?? '',
    kind: preset?.kind ?? 'openai',
    base_url: local?.base_url ?? preset?.base_url ?? '',
    model: local?.models[0]?.id ?? preset?.default_model ?? '',
    effort: '',
    tool_mode: 'auto',
    context_window: '',
    is_default: true,
    enabled: true,
  };
}

/**
 * Adding or editing a model endpoint.
 *
 * Two things here are not decoration. The key is write-only, so it is a
 * "set / not set" field that can only be replaced or cleared. And *Test* makes
 * one real request including one tool call, because whether the model can use
 * a tool decides whether it can look at the network at all — a model that
 * cannot is still worth having, and says so, but it is a different thing.
 */
export function ProviderDrawer({ open, onClose, provider, preset, local }: ProviderDrawerProps) {
  const [draft, setDraft] = useState<Draft>(() => draftOf(provider, preset, local));
  const [apiKey, setApiKey] = useState<string | undefined>(undefined);
  const [models, setModels] = useState<ModelInfo[] | null>(local?.models ?? null);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [listing, setListing] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<ProviderTestResult | null>(null);
  /**
   * *Save and test* really does save: the key has to be in the vault before
   * the backend can make the request. From then on this drawer is editing a
   * saved endpoint, and says so.
   */
  const [saved, setSaved] = useState<ProviderEntry | null>(null);
  const entry = saved ?? provider;

  useEffect(() => {
    if (!open) return;
    setDraft(draftOf(provider, preset, local));
    setApiKey(undefined);
    setModels(local?.models ?? null);
    setModelsError(null);
    setResult(null);
    setSaved(null);
  }, [open, provider, preset, local]);

  const save = useRpcMutation<ProviderEntry, AiProvidersSaveParams>('ai.providers.save', {
    invalidates: ['ai.'],
    errorToast: 'The model was not saved',
  });

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) =>
    setDraft((current) => ({ ...current, [key]: value }));

  const params = (): AiProvidersSaveParams => ({
    ...(entry ? { id: entry.id } : {}),
    name: draft.name.trim() || undefined,
    kind: draft.kind,
    base_url: draft.base_url.trim(),
    model: draft.model.trim(),
    ...(apiKey === undefined ? {} : { api_key: apiKey }),
    ...(draft.effort ? { effort: draft.effort as Effort } : {}),
    tool_mode: draft.tool_mode,
    ...(draft.context_window ? { context_window: Number(draft.context_window) } : {}),
    is_default: draft.is_default,
    enabled: draft.enabled,
  });

  const listModels = () => {
    setListing(true);
    setModelsError(null);
    const query = entry
      ? { id: entry.id }
      : { kind: draft.kind, base_url: draft.base_url.trim(), api_key: apiKey };
    void rpc<ModelInfo[]>('ai.providers.models', query)
      .then((found) => setModels(found))
      .catch((failure: RpcError) => setModelsError(failure.message))
      .finally(() => setListing(false));
  };

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      // Testing needs the endpoint saved: the key lives in the vault, and the
      // test is a real request made by the backend, not by this window.
      const stored = await save.mutateAsync(params());
      setSaved(stored);
      setApiKey(undefined);
      const outcome = await rpc<ProviderTestResult>('ai.providers.test', { id: stored.id });
      setResult(outcome);
    } catch {
      // `useRpcMutation` has already said so.
    } finally {
      setTesting(false);
    }
  };

  const submit = async () => {
    try {
      await save.mutateAsync(params());
      toast.success(entry ? 'Model updated' : 'Model added');
      onClose();
    } catch {
      // Reported by the mutation's error toast; the drawer stays open.
    }
  };

  const modelOptions = (models ?? []).map((model) => ({
    value: model.id,
    label: model.name,
    description: model.context_window ? `${formatCount(model.context_window)} tokens` : undefined,
  }));
  // A model typed by hand stays choosable after the endpoint has been asked.
  if (draft.model && !modelOptions.some((option) => option.value === draft.model)) {
    modelOptions.unshift({ value: draft.model, label: draft.model, description: 'typed by hand' });
  }

  return (
    <Drawer
      open={open}
      onClose={onClose}
      title={provider ? `Edit ${provider.name}` : 'Add a model'}
      description={
        provider
          ? 'The API key is stored in the vault and is never shown again.'
          : 'Claude, any OpenAI-compatible endpoint, or a model running on this machine.'
      }
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={save.isPending && !testing} onClick={() => void submit()}>
            {entry ? 'Save' : 'Add model'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Field label="Name" hint="What this endpoint is called in Nocturne.">
          <Input value={draft.name} onChange={(event) => set('name', event.target.value)} />
        </Field>
        <Field label="API" hint="Which protocol the endpoint speaks.">
          <Select
            options={KINDS}
            value={draft.kind}
            onChange={(event) => set('kind', event.target.value as ProviderKind)}
          />
        </Field>
        <Field
          label="Address"
          hint={
            draft.kind === 'anthropic'
              ? 'Leave empty for api.anthropic.com.'
              : 'The base URL, up to and including /v1 where the endpoint expects it.'
          }
        >
          <Input
            mono
            placeholder="http://127.0.0.1:11434"
            value={draft.base_url}
            onChange={(event) => set('base_url', event.target.value)}
          />
        </Field>
        <SecretField
          label="API key"
          hint={
            draft.kind === 'ollama'
              ? 'Ollama usually needs none.'
              : 'Stored in the vault. Nocturne never shows it again.'
          }
          isSet={entry?.has_key ?? false}
          value={apiKey}
          onChange={setApiKey}
          onReset={() => setApiKey(undefined)}
        />
        <Field
          label="Model"
          hint={modelsError ?? 'Ask the endpoint which models it offers, or type the name.'}
          error={modelsError ?? undefined}
        >
          {modelOptions.length > 0 ? (
            <Combobox
              options={modelOptions}
              value={draft.model || null}
              onChange={(value) => set('model', value ?? '')}
              placeholder="Choose a model"
            />
          ) : (
            <Input
              mono
              placeholder="claude-opus-5-5"
              value={draft.model}
              onChange={(event) => set('model', event.target.value)}
            />
          )}
        </Field>
        <div className={styles.formActions}>
          <Button size="sm" icon={RefreshCw} loading={listing} onClick={listModels}>
            List models
          </Button>
          <Button
            size="sm"
            icon={Wrench}
            loading={testing}
            disabled={!draft.model.trim()}
            onClick={() => void test()}
          >
            Save and test
          </Button>
        </div>
        {testing && (
          <p className={styles.testing}>
            <Spinner size={13} label={null} />
            Making one small request, including a tool call…
          </p>
        )}
        {result && <TestResult result={result} />}

        <Field
          label="Effort"
          hint="How hard the model should think, where it has such a control."
        >
          <Select
            options={EFFORTS}
            value={draft.effort}
            onChange={(event) => set('effort', event.target.value)}
          />
        </Field>
        <Field
          label="Tools"
          hint="How the tools are offered. Local models often need them in the prompt."
        >
          <Select
            options={TOOL_MODES}
            value={draft.tool_mode}
            onChange={(event) => set('tool_mode', event.target.value as ToolMode)}
          />
        </Field>
        <Field
          label="Context window"
          hint="Tokens. Under 16 000 the assistant offers a smaller set of tools and shorter results."
        >
          <Input
            inputMode="numeric"
            placeholder="from the endpoint"
            value={draft.context_window}
            onChange={(event) => set('context_window', event.target.value.replace(/\D/g, ''))}
          />
        </Field>
        <Field label="Use this one by default" hint="New chats start with the default model.">
          <Switch
            checked={draft.is_default}
            onChange={(event) => set('is_default', event.target.checked)}
          />
        </Field>
        <Field label="Enabled" hint="A disabled model is kept but never used.">
          <Switch
            checked={draft.enabled}
            onChange={(event) => set('enabled', event.target.checked)}
          />
        </Field>
      </div>
    </Drawer>
  );
}

/** What one real request found out — above all, whether tools work. */
export function TestResult({ result }: { result: ProviderTestResult }) {
  const tools = TOOL_CALLING[result.tool_calling];
  return (
    <div className={styles.testResult} data-ok={result.ok || undefined}>
      <p className={styles.testHeadline}>
        <Icon icon={result.ok ? CircleCheck : OctagonExclamationPoint} size={14} />
        {result.ok ? `${result.model} answered in ${result.latency_ms} ms` : 'It did not answer'}
      </p>
      {result.ok && (
        <p className={styles.testTools} data-tone={tools.tone}>
          <strong>{tools.title}</strong> {tools.detail}
        </p>
      )}
      {result.error && <p className={styles.testDetail}>{result.error}</p>}
    </div>
  );
}
