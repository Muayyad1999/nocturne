/**
 * Preparing a dialog when it opens, and at no other time.
 *
 * A dialog here empties its form, and forgets the refusal the last attempt
 * got, when it is opened. Written as an effect over what the dialog was
 * given, that also ran whenever one of those things changed *while it was
 * open* — and the invoice a dialog is given changes at exactly the wrong
 * moment. A save waits for the lists behind it to be read again before it
 * reports back (`api/hooks.ts: useRpcMutation`), the invoice is one of those
 * reads, and whenever it answered before the slowest of the others the form
 * was emptied and the save's own answer was thrown away with it: resetting a
 * mutation detaches the answer that is still on its way. The credit note had
 * been issued, and the dialog stood open and blank with no word that it had
 * — asking to be filled in again, which writes the amount off twice. A
 * payment recorded from an invoice went the same way.
 *
 * So the form is prepared on the change from closed to open, and what the
 * dialog was given is read at that moment.
 */

import { useEffect, useEffectEvent, useRef } from 'react';

export function useOnOpen(open: boolean, prepare: () => void): void {
  const wasOpen = useRef(false);
  const onOpen = useEffectEvent(prepare);
  useEffect(() => {
    const opening = open && !wasOpen.current;
    wasOpen.current = open;
    if (opening) onOpen();
  }, [open]);
}
