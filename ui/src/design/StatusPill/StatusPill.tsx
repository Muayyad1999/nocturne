import {
  CircleCheck,
  CircleDashed,
  FlaskConical,
  Info,
  OctagonExclamationPoint,
  TriangleExclamationPoint,
  Wrench,
} from 'lucide-react';
import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import { Icon, type LucideIcon } from '../Icon';
import { STATUS_LABEL, type StatusTone } from '../tone';
import styles from './StatusPill.module.css';

const ICONS: Record<StatusTone, LucideIcon> = {
  ok: CircleCheck,
  warn: TriangleExclamationPoint,
  crit: OctagonExclamationPoint,
  info: Info,
  idle: CircleDashed,
  maint: Wrench,
};

export interface StatusPillProps extends ComponentProps<'span'> {
  tone: StatusTone;
  /** Replaces the tone's default icon. */
  icon?: LucideIcon;
}

/**
 * A state as an icon and a word: `Up`, `Degraded`, `Down`. Without children it
 * shows the tone's default word.
 */
export function StatusPill({ tone, icon, className, children, ...rest }: StatusPillProps) {
  return (
    <span {...rest} className={cx(styles.pill, className)} data-tone={tone}>
      <Icon icon={icon ?? ICONS[tone]} size={12} />
      {children ?? STATUS_LABEL[tone]}
    </span>
  );
}

export type LabPillProps = Omit<ComponentProps<'span'>, 'children'>;

/**
 * Marks a lab or simulated device. It goes everywhere such a device appears,
 * so a simulation can never be mistaken for production.
 */
export function LabPill({ title = 'Lab device: simulated, not production', ...rest }: LabPillProps) {
  return (
    <StatusPill {...rest} tone="maint" icon={FlaskConical} title={title}>
      LAB
    </StatusPill>
  );
}
