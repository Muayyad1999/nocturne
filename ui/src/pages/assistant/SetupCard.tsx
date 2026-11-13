import { Cpu, KeyRound, Plug, Sparkles } from 'lucide-react';
import { useRpc } from '@/api/hooks';
import type { LocalEndpoint, ProviderPreset } from '@/api/types/ai';
import { Button } from '@/design/Button';
import { Icon, type LucideIcon } from '@/design/Icon';
import { Spinner } from '@/design/Spinner';
import styles from './Assistant.module.css';

export interface SetupCardProps {
  /** What the backend said when it was asked to start a turn without a model. */
  sentence?: string;
  /** Opens the provider editor, prefilled from a preset. */
  onAdd: (preset: ProviderPreset | null, local?: LocalEndpoint) => void;
}

const DOOR_ICON: Record<string, LucideIcon> = {
  anthropic: Sparkles,
  ollama: Cpu,
  lmstudio: Cpu,
};

/**
 * No model configured. This is a normal state, not an error: the assistant is
 * the one part of Nocturne nothing else depends on, so this card explains and
 * offers the three ways in rather than shouting.
 *
 * `ai.providers.detect_local` is asked first, because the one-click answer —
 * a model already running on this machine — is the best one and needs no key.
 */
export function SetupCard({ sentence, onAdd }: SetupCardProps) {
  const presets = useRpc<ProviderPreset[]>('ai.providers.presets');
  const local = useRpc<LocalEndpoint[]>('ai.providers.detect_local');
  const byId = new Map((presets.data ?? []).map((preset) => [preset.id, preset]));
  const claude = byId.get('anthropic');
  const openai = byId.get('openai');

  return (
    <div className={styles.setup}>
      <span className={styles.setupIcon}>
        <Icon icon={Sparkles} size={20} />
      </span>
      <h2 className={styles.setupTitle}>No model is connected yet</h2>
      {sentence && <p className={styles.setupText}>{sentence}</p>}
      <p className={styles.setupText}>
        Until one is, the assistant cannot answer. Everything else in Nocturne works without it.
      </p>

      <div className={styles.doors}>
        {local.isPending && (
          <p className={styles.looking}>
            <Spinner size={13} label={null} />
            Looking for a model on this machine…
          </p>
        )}
        {(local.data ?? []).map((endpoint) => {
          const preset = byId.get(endpoint.preset);
          return (
            <div key={endpoint.base_url} className={styles.door} data-found>
              <Icon icon={DOOR_ICON[endpoint.preset] ?? Cpu} size={14} />
              <div className={styles.doorText}>
                <strong>{preset?.name ?? endpoint.preset} is running here</strong>
                <span>
                  {endpoint.base_url} · {endpoint.models.length}{' '}
                  {endpoint.models.length === 1 ? 'model' : 'models'}. Nothing leaves your network.
                </span>
              </div>
              <Button
                size="sm"
                variant="ai"
                onClick={() => onAdd(preset ?? null, endpoint)}
              >
                Use it
              </Button>
            </div>
          );
        })}
        {local.isSuccess && local.data.length === 0 && (
          <div className={styles.door}>
            <Icon icon={Cpu} size={14} />
            <div className={styles.doorText}>
              <strong>Use a local model</strong>
              <span>
                Nothing is answering on this machine's Ollama or LM Studio ports. Start one and
                look again, or point Nocturne at another address.
              </span>
            </div>
            <Button size="sm" onClick={() => void local.refetch()} loading={local.isFetching}>
              Look again
            </Button>
          </div>
        )}
        {claude && (
          <div className={styles.door}>
            <Icon icon={Sparkles} size={14} />
            <div className={styles.doorText}>
              <strong>{claude.name}</strong>
              <span>{claude.note}</span>
            </div>
            <Button size="sm" icon={KeyRound} onClick={() => onAdd(claude)}>
              Add a key
            </Button>
          </div>
        )}
        {openai && (
          <div className={styles.door}>
            <Icon icon={Plug} size={14} />
            <div className={styles.doorText}>
              <strong>An OpenAI-compatible endpoint</strong>
              <span>OpenAI, OpenRouter, Gemini, Groq, vLLM — anything that speaks that API.</span>
            </div>
            <Button size="sm" onClick={() => onAdd(openai)}>
              Add an endpoint
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
