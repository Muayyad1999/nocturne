/**
 * The name of a new view.
 *
 * This used to be `window.prompt`, which the macOS WebView does not implement:
 * it shows nothing and answers as if the user had cancelled, so "Save as a new
 * view…" did nothing at all there. A name is asked for the way the rest of
 * the application asks for one — in its own dialog.
 */

import { useState } from 'react';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';

export interface SaveViewDialogProps {
  open: boolean;
  /** Backing out: nothing is saved. */
  onClose: () => void;
  /**
   * Called with the name, trimmed and never empty. The dialog stays up while
   * the save is on its way; the page closes it once the view is stored.
   */
  onSave: (name: string) => void;
  /** The save is on its way to the backend. */
  saving: boolean;
}

export function SaveViewDialog({ open, onClose, onSave, saving }: SaveViewDialogProps) {
  const [name, setName] = useState('');

  // Every new view starts from an empty name, not from the last one typed.
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setName('');
  }

  const trimmed = name.trim();
  const save = () => {
    if (trimmed === '' || saving) return;
    onSave(trimmed);
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="sm"
      title="Save as a new view"
      description="The layer, the filters and whether endpoints are drawn are saved under this name."
      dismissible={!saving}
      footer={
        <>
          <Button onClick={onClose} disabled={saving}>
            Cancel
          </Button>
          <Button variant="primary" disabled={trimmed === ''} loading={saving} onClick={save}>
            Save view
          </Button>
        </>
      }
    >
      <Field label="Name">
        <Input
          data-autofocus
          autoComplete="off"
          value={name}
          placeholder="Core and distribution"
          onChange={(event) => setName(event.target.value)}
          onKeyDown={(event) => {
            // An input method ends each composed word with an Enter of its
            // own; that one finishes the word, not the dialog.
            if (event.key === 'Enter' && !event.nativeEvent.isComposing) save();
          }}
        />
      </Field>
    </Dialog>
  );
}
