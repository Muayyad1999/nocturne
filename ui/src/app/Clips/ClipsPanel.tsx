/**
 * Clips: the last things copied through a control that asked for them to be
 * remembered (`copyText(text, { clip })`, see `lib/clipboard.ts`).
 *
 * The panel is a view of that store and adds nothing to it. It never reads
 * the system clipboard, and what it shows lives in this window's memory
 * only: a reload empties it, and so does Clear.
 *
 * What can be done with a clip beyond copying it again depends on where the
 * panel is shown, so those actions are handed in: a screen with a terminal
 * gives `onType`, a build with evidence gives `onKeep`, and so on. An action
 * that is not given is not drawn — there is no dead button.
 *
 * A clip's text can be device output. It is shown as text, one line of it,
 * and never written into anything that would interpret it.
 */

import { ClipboardCopy, FileCheck2, Keyboard, ScrollText, X } from 'lucide-react';
import { useSyncExternalStore } from 'react';
import { Button, IconButton } from '@/design/Button';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { clips, copyText, type Clip } from '@/lib/clipboard';
import { plural } from '@/lib/format';
import styles from './ClipsPanel.module.css';

/** The clips, newest first, as a component reads them. */
export function useClips(): readonly Clip[] {
  return useSyncExternalStore(clips.subscribe, clips.list, clips.list);
}

/** The first line of a clip and how many lines it has: enough to tell clips apart. */
export function clipPreview(text: string): { first: string; lines: number } {
  const lines = text.replace(/\r\n?/g, '\n').replace(/\n$/, '').split('\n');
  return { first: lines[0] ?? '', lines: lines.length };
}

export interface ClipsPanelProps {
  /** Types the clip into a terminal session — through the paste confirmation. Given only where there is a terminal. */
  onType?: (clip: Clip) => void;
  /** The session `onType` would type into, by its title; null when none is in front. */
  typeInto?: string | null;
  /** Opens "keep as evidence" on the clip's text. Given only in a build with evidence. */
  onKeep?: (clip: Clip) => void;
  /** Starts a new snippet from the clip's text. */
  onSnippet?: (clip: Clip) => void;
}

export function ClipsPanel({ onType, typeInto, onKeep, onSnippet }: ClipsPanelProps) {
  const held = useClips();

  const copyAgain = async (clip: Clip) => {
    // Asked for again by its own control: it stays a clip, and moves to the front.
    const copied = await copyText(clip.text, { clip: { label: clip.label, source: clip.source } });
    if (copied) toast.success('Copied again');
    else toast.error('The clipboard refused the copy');
  };

  return (
    <section className={styles.panel} aria-label="Clips">
      <header className={styles.header}>
        <h2 className={styles.title}>Clips</h2>
        <span className={styles.count}>{held.length === 0 ? 'none' : plural(held.length, 'clip')}</span>
        <span className={styles.spacer} />
        <Button size="sm" variant="ghost" disabled={held.length === 0} onClick={() => clips.clear()}>
          Clear
        </Button>
      </header>

      {held.length === 0 ? (
        <p className={styles.empty}>
          Nothing has been copied through a button that keeps a clip. A selection copied in a terminal lands here.
        </p>
      ) : (
        <ul className={styles.list}>
          {held.map((clip, index) => {
            const preview = clipPreview(clip.text);
            return (
              <li key={clip.id} className={styles.clip}>
                <div className={styles.what}>
                  <span className={styles.label}>{clip.label}</span>
                  <span className={styles.source}>
                    {clip.source} · <TimeAgo at={clip.at} />
                  </span>
                </div>
                <code className={styles.text}>{preview.first === '' ? ' ' : preview.first}</code>
                {preview.lines > 1 && <span className={styles.more}>{preview.lines} lines</span>}
                <div className={styles.actions}>
                  <IconButton
                    icon={ClipboardCopy}
                    size="sm"
                    aria-label={`Copy “${clip.label}” again`}
                    tooltip="Copy again"
                    // Where the focus lands when the panel opens: the newest
                    // clip, on what changes nothing — not Clear, the first
                    // control in the panel, which a second Enter would press.
                    data-autofocus={index === 0 || undefined}
                    onClick={() => void copyAgain(clip)}
                  />
                  {onType && (
                    <IconButton
                      icon={Keyboard}
                      size="sm"
                      aria-label={`Type “${clip.label}” into ${typeInto ?? 'a session'}`}
                      tooltip={typeInto ? `Type into ${typeInto} (asked first when it is worth asking)` : 'Open a session to type it into'}
                      disabled={!typeInto}
                      onClick={() => onType(clip)}
                    />
                  )}
                  {onKeep && (
                    <IconButton
                      icon={FileCheck2}
                      size="sm"
                      aria-label={`Keep “${clip.label}” as evidence`}
                      tooltip="Keep as evidence…"
                      onClick={() => onKeep(clip)}
                    />
                  )}
                  {onSnippet && (
                    <IconButton
                      icon={ScrollText}
                      size="sm"
                      aria-label={`Save “${clip.label}” as a snippet`}
                      tooltip="Save as a snippet…"
                      onClick={() => onSnippet(clip)}
                    />
                  )}
                  <IconButton
                    icon={X}
                    size="sm"
                    aria-label={`Forget “${clip.label}”`}
                    tooltip="Forget this clip"
                    onClick={() => clips.remove(clip.id)}
                  />
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <p className={styles.foot}>
        Kept in this window’s memory only: not stored, not sent anywhere, gone on reload. Only buttons that say so add
        a clip; the system clipboard is never read.
      </p>
    </section>
  );
}
