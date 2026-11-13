import {
  CircleCheck,
  Info,
  OctagonExclamationPoint,
  TriangleExclamationPoint,
  X,
} from 'lucide-react';
import { useEffect, useState } from 'react';
import { Button, IconButton } from '../Button';
import { Icon, type LucideIcon } from '../Icon';
import { Portal } from '../overlay/Portal';
import type { StatusTone } from '../tone';
import styles from './Toast.module.css';
import { toast, useToasts, type ToastItem, type ToastTone } from './toasts';

/** How many toasts are on screen at once; older ones wait their turn. */
const MAX_VISIBLE = 4;

export const TOAST_APPEARANCE: Record<ToastTone, { icon: LucideIcon; tone: StatusTone }> = {
  info: { icon: Info, tone: 'info' },
  success: { icon: CircleCheck, tone: 'ok' },
  warning: { icon: TriangleExclamationPoint, tone: 'warn' },
  error: { icon: OctagonExclamationPoint, tone: 'crit' },
};

function ToastCard({ item }: { item: ToastItem }) {
  const [paused, setPaused] = useState(false);
  const { icon, tone } = TOAST_APPEARANCE[item.tone];
  const urgent = item.tone === 'error' || item.tone === 'warning';

  useEffect(() => {
    if (item.duration === null || paused) return;
    const timer = setTimeout(() => toast.dismiss(item.id), item.duration);
    return () => clearTimeout(timer);
  }, [item.id, item.duration, paused]);

  return (
    <div
      className={styles.toast}
      role={urgent ? 'alert' : 'status'}
      data-tone={tone}
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={() => setPaused(false)}
    >
      <Icon icon={icon} className={styles.icon} />
      <div className={styles.body}>
        <p className={styles.title}>{item.title}</p>
        {item.description && <p className={styles.description}>{item.description}</p>}
      </div>
      {item.action && (
        <Button
          size="sm"
          onClick={() => {
            item.action?.onSelect();
            toast.dismiss(item.id);
          }}
        >
          {item.action.label}
        </Button>
      )}
      <IconButton
        icon={X}
        size="sm"
        aria-label="Dismiss"
        tooltip={false}
        onClick={() => toast.dismiss(item.id)}
      />
    </div>
  );
}

/** The toast stack, bottom right. Rendered once by the app shell. */
export function Toaster() {
  const toasts = useToasts((state) => state.toasts);
  return (
    <Portal>
      <div className={styles.viewport} role="region" aria-label="Notifications">
        {toasts.slice(-MAX_VISIBLE).map((item) => (
          <ToastCard key={item.id} item={item} />
        ))}
      </div>
    </Portal>
  );
}
