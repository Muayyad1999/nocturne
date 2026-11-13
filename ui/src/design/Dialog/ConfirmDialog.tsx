import { useState, type ReactNode } from 'react';
import { Button } from '../Button';
import { Field } from '../Field';
import { Input } from '../Input';
import { Dialog } from './Dialog';

export interface ConfirmDialogProps {
  open: boolean;
  /** Called when the user backs out. */
  onClose: () => void;
  onConfirm: () => void;
  /** The question, naming the action and its object: "Delete 3 backups?". */
  title: string;
  /** Exactly what will happen, and on which devices. */
  description?: ReactNode;
  /** Further detail, such as the list of affected devices. */
  children?: ReactNode;
  /** The verb of the action, not "OK": "Delete", "Reboot", "Apply". */
  confirmLabel: string;
  cancelLabel?: string;
  /** `danger` makes the confirm button red: for anything destructive or device-touching. */
  tone?: 'default' | 'danger';
  /**
   * For actions that cannot be undone: the user must type this text (a device
   * name, a count) before the confirm button unlocks.
   */
  confirmText?: string;
  /** The action is running: the confirm button shows it and the dialog stays. */
  pending?: boolean;
}

/**
 * Asks before something consequential happens. Cancel has the initial focus,
 * so a stray Enter never confirms a destructive action.
 */
export function ConfirmDialog({
  open,
  onClose,
  onConfirm,
  title,
  description,
  children,
  confirmLabel,
  cancelLabel = 'Cancel',
  tone = 'default',
  confirmText,
  pending = false,
}: ConfirmDialogProps) {
  const [typed, setTyped] = useState('');
  const unlocked = confirmText === undefined || typed.trim() === confirmText;

  // Forget what was typed whenever the dialog opens again.
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setTyped('');
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={title}
      description={description}
      size="sm"
      dismissible={!pending}
      footer={
        <>
          <Button onClick={onClose} disabled={pending} data-autofocus={confirmText === undefined || undefined}>
            {cancelLabel}
          </Button>
          <Button
            variant={tone === 'danger' ? 'danger' : 'primary'}
            disabled={!unlocked}
            loading={pending}
            onClick={onConfirm}
          >
            {confirmLabel}
          </Button>
        </>
      }
    >
      {children}
      {confirmText !== undefined && (
        <Field
          label={
            <>
              Type <code>{confirmText}</code> to confirm
            </>
          }
        >
          <Input
            mono
            data-autofocus
            autoComplete="off"
            spellCheck={false}
            value={typed}
            onChange={(event) => setTyped(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && unlocked && !pending) onConfirm();
            }}
          />
        </Field>
      )}
    </Dialog>
  );
}
