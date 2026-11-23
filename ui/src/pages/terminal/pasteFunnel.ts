/**
 * The one road pasted text takes to a device.
 *
 * Text arrives by many doors — the browser's own paste (Shift+Insert,
 * Ctrl+Shift+V, ⌘V, a middle click, the Edit menu), the toolbar, the context
 * menu, a snippet, a drop — and they all end here. The funnel asks the
 * device's policy about the text, puts the question to the person when it is
 * worth one, and only then hands the bytes over: once, with the line endings
 * a keyboard would have produced.
 *
 * It exists because the doors used to disagree. The browser's own paste was
 * sent by xterm the instant it happened and again when the sheet was
 * confirmed; the others sent whatever line endings the clipboard held.
 *
 * No React and no xterm in here: what a session is, how the policy is asked,
 * how the question is shown and how bytes are sent are all handed in, so the
 * order of events can be tested without any of them.
 */

import type { TerminalLineClass } from '@/api/types/terminal';
import { asTyped, bracketed, needsConfirmation, normaliseLines, type PasteSource } from './paste';

export type { PasteSource } from './paste';

/** What the funnel needs to know about where the text is going. */
export interface PasteTarget {
  /** The session's title, for the question. */
  title: string;
  /** The inventory device whose policy judges the lines; the generic policy without one. */
  deviceId: string | null;
  /** How many sessions the text would reach: more than one while typing is broadcast. */
  targets: number;
  /** The program at the far end has asked for pastes to be bracketed (DECSET 2004). */
  bracketedPaste: boolean;
}

/** A paste that is worth asking about, as the person is shown it. */
export interface PasteQuestion {
  sessionId: string;
  title: string;
  /** The text with every line ending as `\n`. */
  text: string;
  /** What `terminal.classify` said, or null when it could not be asked. */
  classes: TerminalLineClass[] | null;
  source: PasteSource;
  targets: number;
}

/**
 * `sent`: the device has it. `cancelled`: the person said no. `empty`: there
 * was nothing to send. `gone`: the session went away first.
 */
export type PasteOutcome = 'sent' | 'cancelled' | 'empty' | 'gone';

export interface PasteFunnelDeps {
  /** The session as it is now, or null when it has gone. */
  target(sessionId: string): Promise<PasteTarget | null>;
  /** `terminal.classify`. A rejection means the policy could not be asked. */
  classify(deviceId: string | null, text: string): Promise<TerminalLineClass[]>;
  /** Puts the question to the person; true sends. */
  ask(question: PasteQuestion): Promise<boolean>;
  /** Hands bytes to the session, exactly as given. */
  send(sessionId: string, data: string): void;
}

interface Offer {
  sessionId: string;
  text: string;
  source: PasteSource;
  /** Still being judged: what is typed for the session meanwhile waits in `keys`. */
  judging: boolean;
  keys: string;
}

export class PasteFunnel {
  /** One paste is asked about at a time; the next waits for this. */
  private tail: Promise<unknown> = Promise.resolve();
  private offers: Offer[] = [];
  /** A question is on screen, so no key can be meant for a terminal. */
  private asking = false;

  constructor(private readonly deps: PasteFunnelDeps) {}

  /**
   * Text that someone wants in a session. Resolves when it has been sent or
   * given up; nothing reaches the device before that.
   */
  offer(sessionId: string, text: string, source: PasteSource = 'paste'): Promise<PasteOutcome> {
    const offer: Offer = { sessionId, text, source, judging: true, keys: '' };
    this.offers.push(offer);
    const outcome = this.tail.then(() => this.handle(offer));
    // A paste that failed must not stand in the way of every later one.
    this.tail = outcome.catch(() => undefined);
    return outcome;
  }

  /**
   * Keystrokes for a session, on their way to it. Returns true when the
   * funnel has kept them: a paste for that session is still being judged, and
   * what is typed on its heels — the Enter after a pasted command — has to
   * arrive behind it, not ahead of it because the policy took a moment.
   *
   * Kept keys are sent after the paste, and thrown away with a paste that is
   * refused: they were typed blind, to follow text that never arrived.
   */
  typed(sessionId: string, data: string): boolean {
    // While a question is open the keyboard is the question's. What arrives
    // for a session then is the terminal answering its device (a cursor
    // report, a focus event), and that must not wait for a person — not even
    // behind a second paste that is queued for its turn.
    if (this.asking) return false;
    for (let index = this.offers.length - 1; index >= 0; index -= 1) {
      const offer = this.offers[index];
      if (offer && offer.sessionId === sessionId && offer.judging) {
        offer.keys += data;
        return true;
      }
    }
    return false;
  }

  private async handle(offer: Offer): Promise<PasteOutcome> {
    const { deps } = this;
    const { sessionId, source } = offer;
    try {
      let text = normaliseLines(offer.text);
      // Never an Enter of ours: the newline that ends a snippet comes off.
      if (source === 'snippet') text = text.replace(/\n$/, '');
      if (text === '') return 'empty';

      const first = await deps.target(sessionId);
      if (!first) return 'gone';

      let classes: TerminalLineClass[] | null;
      try {
        classes = await deps.classify(first.deviceId, text);
      } catch {
        // A policy that cannot be asked must not stop a paste; the sheet says so.
        classes = null;
      }

      // Read again: a broadcast may have been switched on while the policy answered.
      const judged = await deps.target(sessionId);
      if (!judged) return 'gone';

      if (judged.targets > 1 || needsConfirmation(text, classes)) {
        offer.judging = false;
        this.asking = true;
        let confirmed: boolean;
        try {
          confirmed = await deps.ask({
            sessionId,
            title: judged.title,
            text,
            classes,
            source,
            targets: judged.targets,
          });
        } finally {
          this.asking = false;
        }
        if (!confirmed) return 'cancelled';
      }

      // And once more: the question can stay open for as long as the person
      // likes, and the session, or what it is running, may have changed.
      const now = await deps.target(sessionId);
      if (!now) return 'gone';

      let data = asTyped(text);
      // Only for the one session that asked. To a device that did not, the
      // markers are six characters typed at its prompt.
      if (now.bracketedPaste && now.targets <= 1) data = bracketed(data);
      deps.send(sessionId, data);
      if (offer.keys !== '') deps.send(sessionId, offer.keys);
      return 'sent';
    } finally {
      offer.judging = false;
      this.offers = this.offers.filter((entry) => entry !== offer);
    }
  }
}
