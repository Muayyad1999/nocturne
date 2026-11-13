/**
 * The device palette: every model `labs.catalog` offers, as a button that
 * puts one on the canvas. The list is the backend's — nothing here knows what
 * a simulated device is, only what the simulator says it has.
 */

import { Cable } from 'lucide-react';
import type { ModelInfo } from '@/api/types/lab';
import { Button } from '@/design/Button';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Skeleton } from '@/design/Skeleton';
import { Tooltip } from '@/design/Tooltip';
import type { RpcError } from '@/api/errors';
import type { CableTool } from './cabling';
import { kindIcon, shortModel } from './model';
import styles from './Palette.module.css';

export interface PaletteProps {
  models: ModelInfo[] | undefined;
  loading: boolean;
  error: RpcError | null;
  onRetry: () => void;
  onAdd: (model: ModelInfo) => void;
  adding: string | null;
  cable: CableTool;
  onCable: () => void;
  disabled: boolean;
}

export function Palette({
  models,
  loading,
  error,
  onRetry,
  onAdd,
  adding,
  cable,
  onCable,
  disabled,
}: PaletteProps) {
  return (
    <div className={styles.palette}>
      <h2 className={styles.heading}>Add</h2>

      {loading && (
        <div className={styles.models}>
          {Array.from({ length: 8 }, (_value, index) => (
            <Skeleton key={index} height={46} />
          ))}
        </div>
      )}

      {error && (
        <ErrorState title="The catalog could not be read" error={error} onRetry={onRetry} size="compact" />
      )}

      {models && (
        <div className={styles.models} role="group" aria-label="Device models">
          {models.map((model) => (
            <Tooltip key={model.id} content={`${model.label} — ${model.description}`} placement="right">
              <button
                type="button"
                className={styles.model}
                aria-label={`Add ${model.label}`}
                disabled={disabled || adding !== null}
                data-busy={adding === model.id || undefined}
                onClick={() => onAdd(model)}
              >
                <Icon icon={kindIcon(model.kind)} size={18} />
                <span className={styles.modelName}>{shortModel(model.id, model.label)}</span>
              </button>
            </Tooltip>
          ))}
        </div>
      )}

      <h2 className={styles.heading}>Connect</h2>
      <Button
        variant={cable.mode === 'off' ? 'secondary' : 'primary'}
        size="sm"
        icon={Cable}
        className={styles.cable}
        aria-pressed={cable.mode !== 'off'}
        disabled={disabled}
        onClick={onCable}
      >
        <span className={styles.cableLabel}>Cable</span>
      </Button>
    </div>
  );
}
