/**
 * "Keep as evidence…" for anything the engine can capture by itself.
 *
 * A screen that shows a chart or the map can offer to keep what is on it. The
 * dialog here does not send the picture or the numbers: it sends *what to
 * read* (`evidence.capture_chart`, `evidence.capture_topology`) and the
 * engine reads it again, redacts it and stores it with its source and a
 * hash. That is why the dialog can say where the item will come from before
 * it exists, and why what is kept is checkable afterwards.
 *
 * How a screen uses it:
 *
 *   const keep = useKeepAsEvidence();
 *   …
 *   <TimeSeriesChart
 *     …
 *     exportMenu={{ extra: keep.chartEntries(() => ({ kind: 'chart', series, since, until: Date.now(), step_s })) }}
 *   />
 *   {keep.dialog}
 *
 * `keep.can('chart')` is false in a build without `evidence.capture_chart`
 * (and while `app.methods` is still on its way), and `chartEntries` is then
 * empty, so a build without evidence shows no entry at all. For the map,
 * show a button when `keep.can('topology')` and call `keep.keep({ kind:
 * 'topology', view, layer, filter, include_endpoints })`.
 *
 * It reads with `useRpc` and writes with a plain `rpc` call rather than a
 * mutation hook: the lists that show evidence refresh on the engine's own
 * `evidence.changed` and `incidents.changed`, so there is nothing to
 * invalidate by hand.
 */

import { FileCheck2 } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { rpc } from '@/api/client';
import { useRpc } from '@/api/hooks';
import type { Evidence, EvidenceWithBody, IncidentsListResult } from '@/api/types/ops';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { KeyValue } from '@/design/KeyValue';
import type { MenuAction } from '@/design/Menu';
import { Select } from '@/design/Select';
import { toast } from '@/design/Toast';
import { useAuthor } from '@/pages/ops/author';
import styles from './KeepAsEvidence.module.css';
import {
  CAPTURE_METHOD,
  captureCall,
  describeRequest,
  evidenceLink,
  refusal,
  type ChartEvidenceRequest,
  type KeepRequest,
  type Refusal,
  type TopologyEvidenceRequest,
} from './keep';

export type { ChartEvidenceRequest, KeepRequest, TopologyEvidenceRequest };

/**
 * The methods this build answers, or an empty set while that is not known.
 * One read for the four names this module asks about.
 */
function useMethods(): ReadonlySet<string> {
  const methods = useRpc<string[]>('app.methods');
  // Anything but a list of names is "not known": nothing is offered on a guess.
  return new Set(Array.isArray(methods.data) ? methods.data : []);
}

const ABOUT: Record<KeepRequest['kind'], { what: string; sentence: string }> = {
  chart: {
    what: 'these samples',
    sentence:
      'The engine reads these samples again from its own store and keeps the points it finds — with their gaps left as gaps — as a record with a source, a time and a hash. Nothing from this screen is uploaded.',
  },
  topology: {
    what: 'this drawing',
    sentence:
      'The engine draws this graph again and keeps its own drawing as a record with a source, a time and a hash. It is the engine’s drawing of the same graph, not a pixel copy of the screen.',
  },
};

export interface KeepAsEvidenceDialogProps {
  /** What to keep. `null` renders nothing. */
  request: KeepRequest | null;
  onClose: () => void;
  /** Called once the item is stored, with the item as the engine returned it (without its body). */
  onKept?: (item: Evidence) => void;
  /** An incident to offer first in "Attach to", e.g. the one the screen is about. */
  incidentId?: string;
}

/** The dialog alone, for a screen that keeps its own state. Most screens want {@link useKeepAsEvidence}. */
export function KeepAsEvidenceDialog({ request, onClose, onKept, incidentId }: KeepAsEvidenceDialogProps) {
  if (request === null) return null;
  // Keyed by what is asked for, so a second request starts with an empty form.
  return (
    <Form key={JSON.stringify(request)} request={request} onClose={onClose} onKept={onKept} incidentId={incidentId} />
  );
}

interface FormProps extends KeepAsEvidenceDialogProps {
  request: KeepRequest;
}

function Form({ request, onClose, onKept, incidentId }: FormProps) {
  const methods = useMethods();
  const canAttach = methods.has('evidence.attach') && methods.has('incidents.list');
  const incidents = useRpc<IncidentsListResult>(
    'incidents.list',
    { state: 'unresolved', limit: 200 },
    { enabled: canAttach, invalidateOn: ['incidents.changed'] },
  );

  const [title, setTitle] = useState(request.title ?? '');
  const [attachTo, setAttachTo] = useState(incidentId ?? '');
  const [author, setAuthor] = useAuthor();
  const [pending, setPending] = useState(false);
  const [refused, setRefused] = useState<Refusal | null>(null);
  /** Stored, but the attachment that was asked for did not happen. */
  const [unattached, setUnattached] = useState<{ item: Evidence; why: string } | null>(null);

  const about = ABOUT[request.kind];
  const open = Array.isArray(incidents.data?.incidents) ? incidents.data.incidents : [];

  const finish = (item: Evidence, attached: boolean) => {
    toast.success(attached ? 'Kept as evidence and attached' : 'Kept as evidence', {
      description: `${item.title} · sha256 ${item.sha256.slice(0, 12)}…`,
      action: {
        label: 'Open',
        // A hash route, set directly: this dialog is also used where there is
        // no router above it to ask.
        onSelect: () => {
          window.location.hash = evidenceLink(item.id);
        },
      },
    });
    onKept?.(item);
    onClose();
  };

  const attach = async (item: Evidence) => {
    try {
      const attached = await rpc<Evidence>('evidence.attach', {
        id: item.id,
        incident_id: attachTo,
        ...(author.trim() === '' ? {} : { author: author.trim() }),
      });
      finish(attached, true);
    } catch (error) {
      setUnattached({ item, why: refusal(error).message });
    }
  };

  const keep = async () => {
    setPending(true);
    setRefused(null);
    try {
      const call = captureCall(request, title);
      // The body comes back with the item; it is not kept here.
      const { body: _body, ...item } = await rpc<EvidenceWithBody>(call.method, call.params);
      if (attachTo !== '') await attach(item);
      else finish(item, false);
    } catch (error) {
      setRefused(refusal(error));
    } finally {
      setPending(false);
    }
  };

  if (unattached) {
    return (
      <Dialog
        open
        onClose={onClose}
        title="Kept, but not attached"
        description="The evidence is stored. Attaching it to the incident is what failed."
        size="md"
        footer={
          <>
            <Button variant="ghost" onClick={onClose}>
              Leave it unattached
            </Button>
            <Button
              variant="primary"
              loading={pending}
              onClick={async () => {
                setPending(true);
                await attach(unattached.item);
                setPending(false);
              }}
            >
              Try attaching again
            </Button>
          </>
        }
      >
        <div className={styles.body}>
          <p className={styles.refused} role="alert">
            <b>It could not be attached.</b> {unattached.why}
          </p>
          <p className={styles.note}>
            “{unattached.item.title}” is under Operations → Evidence, where it can be attached later.
          </p>
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog
      open
      onClose={onClose}
      title="Keep as evidence"
      description={about.sentence}
      size="md"
      dismissible={!pending}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={pending}>
            Cancel
          </Button>
          <Button variant="primary" icon={FileCheck2} loading={pending} onClick={() => void keep()}>
            {attachTo === '' ? 'Keep' : 'Keep and attach'}
          </Button>
        </>
      }
    >
      <div className={styles.body}>
        <KeyValue items={describeRequest(request).map((fact) => ({ label: fact.label, value: fact.value }))} />
        <Field label="Title" hint="Left empty, the engine names it after what it read.">
          <Input
            data-autofocus
            value={title}
            maxLength={200}
            placeholder="What this shows, in a few words"
            onChange={(event) => setTitle(event.target.value)}
          />
        </Field>
        {canAttach && (
          <Field
            label="Attach to an incident"
            hint={
              incidents.isError
                ? `The incidents could not be read (${incidents.error.message}); it can be attached later, under Evidence.`
                : 'It goes on that incident’s timeline at the time it was captured, as a reference.'
            }
          >
            <Select value={attachTo} onChange={(event) => setAttachTo(event.target.value)}>
              <option value="">Do not attach</option>
              {open.map((incident) => (
                <option key={incident.id} value={incident.id}>
                  {incident.title}
                </option>
              ))}
              {/* An incident the screen named that is not among the unresolved ones is still a choice. */}
              {incidentId && !open.some((incident) => incident.id === incidentId) && (
                <option value={incidentId}>This incident</option>
              )}
            </Select>
          </Field>
        )}
        {attachTo !== '' && (
          <Field label="Your name" hint="Shown on the timeline entry. Remembered in this browser.">
            <Input
              size="sm"
              value={author}
              placeholder="who is attaching this"
              onChange={(event) => setAuthor(event.target.value)}
            />
          </Field>
        )}
        <p className={styles.note}>
          Secrets are replaced before it is stored, always. What was replaced cannot be recovered from the item.
        </p>
        {refused && (
          <p className={styles.refused} role="alert" data-testid="keep-refused">
            <b>{refused.headline}.</b> {refused.message}
          </p>
        )}
      </div>
    </Dialog>
  );
}

export interface KeepAsEvidence {
  /**
   * Whether this build can keep that kind. False while `app.methods` has not
   * answered: an entry that appears and then vanishes is worse than one that
   * arrives a moment late.
   */
  can: (kind: KeepRequest['kind']) => boolean;
  /** Opens the dialog on a request. Does nothing in a build that cannot keep it. */
  keep: (request: KeepRequest) => void;
  /**
   * The "Keep as evidence…" entry for a chart's export menu, or nothing in a
   * build without `evidence.capture_chart`. Give it a function when part of
   * the request is "now" (a chart whose range runs to the present): it is
   * called when the entry is chosen, not when the menu is built.
   */
  chartEntries: (request: ChartEvidenceRequest | (() => ChartEvidenceRequest)) => MenuAction[];
  /** Render once, anywhere in the screen. */
  dialog: ReactNode;
}

/** State and dialog for "Keep as evidence…", for one screen. */
export function useKeepAsEvidence(options: { incidentId?: string; onKept?: (item: Evidence) => void } = {}): KeepAsEvidence {
  const methods = useMethods();
  const [request, setRequest] = useState<KeepRequest | null>(null);
  const can = (kind: KeepRequest['kind']) => methods.has(CAPTURE_METHOD[kind]);
  const keep = (next: KeepRequest) => {
    if (can(next.kind)) setRequest(next);
  };

  return {
    can,
    keep,
    chartEntries: (wanted) =>
      can('chart')
        ? [
            {
              label: 'Keep as evidence…',
              icon: FileCheck2,
              onSelect: () => keep(typeof wanted === 'function' ? wanted() : wanted),
            },
          ]
        : [],
    dialog: (
      <KeepAsEvidenceDialog
        request={request}
        onClose={() => setRequest(null)}
        onKept={options.onKept}
        incidentId={options.incidentId}
      />
    ),
  };
}
