/**
 * Printing a document the engine rendered, without trusting what is in it.
 *
 * An HTML report is previewed in a frame that is sandboxed with nothing
 * allowed: no script, and an origin of its own, because the document was made
 * from device data. For the same reason this page cannot reach into it —
 * reading `contentWindow.print` of a frame of another origin is a
 * SecurityError in every engine, which is what the Print buttons once did.
 *
 * `allow-same-origin` would make that call work, by putting a document made
 * from device data on the application's own origin. It is not used. The
 * document is printed by a second frame instead, which also has an origin of
 * its own and is allowed one script, ours (`printFrame.js`): the frame says
 * it is ready, is sent the document, and prints itself. The frame starts
 * empty, so the document never passes through a parser that could run
 * anything in it, and its policy below allows no script but that one file
 * and no request at all — a report is self-contained, and one that is not
 * does not get to phone anywhere from here.
 *
 * The preview is allowed nothing, here and on the Reports tab. Nothing on
 * screen can run script; the print frame is not on screen.
 */

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { toast } from '@/design/Toast';
import styles from './PrintFrame.module.css';
// `no-inline`: a file this small would otherwise be built into a `data:`
// address, which is not the application's own and so is refused by its policy.
import scriptPath from './printFrame.js?url&no-inline';

/**
 * What the print frame may do: run its script and open the print dialog
 * (`print()` is a modal). Never `allow-same-origin`.
 */
export const PRINT_SANDBOX = 'allow-scripts allow-modals';

/** How long the frame's script gets to load and say so before printing is called off. */
export const READY_TIMEOUT_MS = 5_000;

function attribute(value: string): string {
  return value.replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;');
}

/**
 * The empty document the print frame is loaded with. Nothing in it comes
 * from a report: only the policy and the script that will receive one.
 */
export function printShell(scriptUrl: string): string {
  const policy = [
    "default-src 'none'",
    // The one file, not `'self'`: nothing else served from here runs either.
    `script-src ${scriptUrl}`,
    "style-src 'unsafe-inline'",
    'img-src data:',
    'font-src data:',
    "base-uri 'none'",
    "form-action 'none'",
  ].join('; ');
  return (
    '<!doctype html><html><head><meta charset="utf-8">' +
    `<meta http-equiv="Content-Security-Policy" content="${attribute(policy)}">` +
    `<script src="${attribute(scriptUrl)}"></script>` +
    '</head><body></body></html>'
  );
}

interface PrintFrameProps {
  html: string;
  onFailed: () => void;
  /** The frame was taken out of the document. */
  onGone: () => void;
}

/** One frame prints one document: a new job is a new frame. */
function PrintFrame({ html, onFailed, onGone }: PrintFrameProps) {
  const frame = useRef<HTMLIFrameElement>(null);
  // Absolute, because the policy names it and the frame's own address is `about:srcdoc`.
  const shell = useMemo(() => printShell(new URL(scriptPath, document.baseURI).href), []);

  useEffect(() => {
    let sent = false;
    // Only the wait for "ready" is timed. Once the document is sent the frame
    // may be inside `print()` for as long as the dialog is open, and a clock
    // running here would call a print the person is still looking at a failure.
    const overdue = window.setTimeout(onFailed, READY_TIMEOUT_MS);

    const onMessage = (event: MessageEvent) => {
      const target = frame.current?.contentWindow;
      if (!target || event.source !== target) return;
      const state = (event.data as { nocturnePrint?: unknown } | null)?.nocturnePrint;
      if (state === 'ready' && !sent) {
        sent = true;
        window.clearTimeout(overdue);
        // `*`: the frame's origin is opaque, so there is none to name. What
        // receives this is the window we made and checked the sender against,
        // still showing the empty document it was loaded with.
        target.postMessage({ nocturnePrint: 'document', html }, '*');
      } else if (state === 'failed') {
        window.clearTimeout(overdue);
        onFailed();
      }
    };

    window.addEventListener('message', onMessage);
    return () => {
      window.clearTimeout(overdue);
      window.removeEventListener('message', onMessage);
    };
  }, [html, onFailed]);

  useEffect(() => {
    const element = frame.current;
    // Asked of the element, not assumed: a development build runs every
    // cleanup once straight after mounting, with the frame still in place.
    return () => {
      if (element && !element.isConnected) onGone();
    };
  }, [onGone]);

  return (
    <iframe
      ref={frame}
      className={styles.frame}
      title="Document being printed"
      aria-hidden="true"
      tabIndex={-1}
      sandbox={PRINT_SANDBOX}
      srcDoc={shell}
    />
  );
}

export interface PrintFrameHandle {
  /** Sends an HTML document to the printer. */
  printDocument: (html: string) => void;
  /** Render it anywhere in the component: it shows nothing, and is what prints. */
  printFrame: ReactNode;
}

/**
 * Print for a component that shows an engine-rendered HTML document. The
 * frame lives until the next document is printed, or for as long as the
 * place it is rendered in: an engine may still be reading from it after
 * `print()` returns.
 *
 * A job ends with its frame. The component that holds this can outlive what
 * it renders — a dialog stays mounted while it is closed — and a job still
 * held then would be given a new frame when the dialog came back: a print
 * that nobody asked for, of a document from the last time.
 */
export function usePrintFrame(): PrintFrameHandle {
  const [job, setJob] = useState<{ id: number; html: string } | null>(null);
  const jobs = useRef(0);

  const printDocument = useCallback((html: string) => {
    jobs.current += 1;
    setJob({ id: jobs.current, html });
  }, []);

  // Only its own job: the frame of the last print leaves as the next one starts.
  const id = job?.id;
  const gone = useCallback(() => setJob((current) => (current?.id === id ? null : current)), [id]);

  const failed = useCallback(() => {
    setJob(null);
    toast.error('This window could not print the document', {
      description: 'Download it and print the file instead.',
    });
  }, []);

  return {
    printDocument,
    printFrame: job ? <PrintFrame key={job.id} html={job.html} onFailed={failed} onGone={gone} /> : null,
  };
}
