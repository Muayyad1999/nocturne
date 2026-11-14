/**
 * The audit trail: every mutation anybody made through Nocturne.
 *
 * This is a **record, not a feed**. Nothing on this tab is editable, nothing
 * here can be deleted from the interface, and no row is assembled out of
 * anything but the entry the engine wrote: the action as its author named it,
 * the sentence it recorded, the device as the inventory has it now, and the
 * records its own detail names. Nothing is inferred — a link exists only
 * where the feature that wrote the entry put an id in its detail.
 *
 * The detail is shown **verbatim**, and that is deliberate. A secret must
 * never reach the audit trail; if one ever did, masking it here would hide a
 * defect in whichever feature wrote it while leaving the secret in the
 * database, in `audit.search`'s answer and in every export. So this screen
 * does not mask: it shows what the record says, and a leak would be as
 * visible as it is serious.
 *
 * Filtering by action or by referenced record is applied in the engine a page
 * at a time, so paging is by `before_id` rather than an offset; a long trail
 * with a narrow filter still fills a page.
 */

import { ChevronDown, ScrollText } from 'lucide-react';
import { useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import type { AuditEntry, Ref } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { LabPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, plural } from '@/lib/format';
import { formatTime, formatTimestamp } from '@/lib/time';
import { useAuditActions, useAuditTrail, useDevices } from './data';
import styles from './Audit.module.css';
import opsStyles from './Ops.module.css';
import { RESOLVABLE_KINDS, actionGroups, actionTone, instant, refLink } from './ops';

/** `?reference=incident:i1` — the filter the incident page links to. */
function parseReference(text: string | null): Ref | null {
  if (!text) return null;
  const at = text.indexOf(':');
  if (at <= 0 || at === text.length - 1) return null;
  return { type: text.slice(0, at), id: text.slice(at + 1) };
}

export function AuditTab() {
  const [search, setSearch] = useSearchParams();
  const [text, setText] = useState('');
  const [action, setAction] = useState('');
  const [sinceText, setSinceText] = useState('');
  const [page, setPage] = useState<number | null>(null);
  const [expanded, setExpanded] = useState<number | null>(null);

  // Two filters can arrive in the address: the incident page links here with
  // a reference, and the handover links a deleted device by its id.
  const reference = parseReference(search.get('reference'));
  const deviceFilter = search.get('device');

  const since = sinceText === '' ? undefined : new Date(sinceText).getTime();
  const actions = useAuditActions();
  const devices = useDevices();
  const trail = useAuditTrail({
    limit: 100,
    ...(text.trim() === '' ? {} : { text: text.trim() }),
    ...(action === '' ? {} : { action }),
    ...(reference ? { reference } : {}),
    ...(deviceFilter ? { device_id: deviceFilter } : {}),
    ...(since !== undefined && Number.isFinite(since) ? { since } : {}),
    ...(page === null ? {} : { before_id: page }),
  });

  const groups = actionGroups(actions.data?.actions ?? []);
  const filtered = Boolean(
    text.trim() !== '' || action !== '' || reference || deviceFilter || sinceText !== '',
  );
  const clearAddressFilters = () => {
    const next = new URLSearchParams(search);
    next.delete('reference');
    next.delete('device');
    setSearch(next, { replace: true });
    setPage(null);
  };

  return (
    <>
      <Toolbar>
        <SearchInput
          aria-label="Search the audit trail"
          value={text}
          onChange={(value) => {
            setText(value);
            setPage(null);
          }}
          placeholder="Search the recorded sentences"
          className={opsStyles.filter}
        />
        <Select
          aria-label="Action"
          size="sm"
          className={opsStyles.filter}
          value={action}
          onChange={(event) => {
            setAction(event.target.value);
            setPage(null);
          }}
        >
          <option value="">Every action</option>
          {groups.map((group) => (
            <optgroup key={group.group} label={`${group.group} (${formatCount(group.count)})`}>
              <option value={group.group}>All of {group.group}</option>
              {group.actions.map((entry) => (
                <option key={entry.action} value={entry.action}>
                  {entry.action} · {formatCount(entry.count)}
                </option>
              ))}
            </optgroup>
          ))}
        </Select>
        <Input
          size="sm"
          type="datetime-local"
          aria-label="Since"
          className={opsStyles.when}
          value={sinceText}
          onChange={(event) => {
            setSinceText(event.target.value);
            setPage(null);
          }}
        />
        <ToolbarSpacer />
        {(reference !== null || deviceFilter !== null) && (
          <Button size="sm" variant="ghost" onClick={clearAddressFilters}>
            Clear the link’s filter
          </Button>
        )}
      </Toolbar>

      <p className={opsStyles.facts}>
        {reference && (
          <span>
            Only entries whose own detail names{' '}
            <b>
              {reference.type} {reference.id}
            </b>
          </span>
        )}
        {deviceFilter && (
          <span>
            Only entries about device <b>{devices.data?.find((device) => device.id === deviceFilter)?.name ?? deviceFilter}</b>
          </span>
        )}
        {actions.data?.truncated && (
          <span className={opsStyles.muted}>
            The action list was counted over the newest 5 000 entries, so a rare action may be
            missing from the menu.
          </span>
        )}
        <span className={opsStyles.muted}>
          Nothing here can be edited or removed. Times are local; hover one for UTC.
        </span>
      </p>

      {trail.isError ? (
        <ErrorState
          title="The audit trail could not be read"
          error={trail.error}
          onRetry={() => void trail.refetch()}
        />
      ) : trail.isPending ? (
        <div aria-busy="true">
          <Skeleton shape="block" height={360} />
        </div>
      ) : trail.data && trail.data.entries.length === 0 ? (
        <EmptyState
          size="compact"
          icon={ScrollText}
          title={filtered ? 'No recorded action matches that' : 'Nothing has been recorded yet'}
          description={
            filtered
              ? 'Widen the filters, or clear the date.'
              : 'Every user-visible change writes one line here: devices added, configurations backed up, changes applied, incidents opened. Nothing read-only is recorded.'
          }
          action={
            filtered ? (
              <Button
                variant="secondary"
                onClick={() => {
                  setText('');
                  setAction('');
                  setSinceText('');
                  clearAddressFilters();
                }}
              >
                Clear the filters
              </Button>
            ) : undefined
          }
        />
      ) : (
        trail.data && (
          <>
            <ol className={styles.trail} aria-label="Recorded actions">
              {trail.data.entries.map((entry) => (
                <Row
                  key={entry.id}
                  entry={entry}
                  open={expanded === entry.id}
                  onToggle={() => setExpanded(expanded === entry.id ? null : entry.id)}
                />
              ))}
            </ol>
            <div className={styles.paging}>
              {page !== null && (
                <Button variant="secondary" onClick={() => setPage(null)}>
                  Back to the newest
                </Button>
              )}
              <span className={opsStyles.muted}>
                {plural(trail.data.entries.length, 'entry', 'entries')} on this page
              </span>
              {trail.data.has_more && trail.data.next_before_id !== null && (
                <Button variant="secondary" onClick={() => setPage(trail.data.next_before_id)}>
                  Older entries
                </Button>
              )}
            </div>
          </>
        )
      )}
    </>
  );
}

function Row({ entry, open, onToggle }: { entry: AuditEntry; open: boolean; onToggle: () => void }) {
  const at = instant(entry.ts);
  const hasDetail = entry.detail !== null && entry.detail !== undefined;
  return (
    <li className={styles.entry} data-testid="audit-entry" data-action={entry.action} data-tone={actionTone(entry.action)}>
      <Tooltip content={formatTimestamp(entry.ts)} describe={false}>
        <time className={styles.when} dateTime={at ? new Date(at).toISOString() : undefined}>
          {at === null ? '—' : formatTime(at)}
        </time>
      </Tooltip>
      <span className={styles.ago}>
        <TimeAgo at={at} />
      </span>
      <code className={styles.action}>{entry.action}</code>
      <div className={styles.body}>
        <p className={styles.message}>{entry.message}</p>
        <p className={styles.refs}>
          {entry.device && (
            <Link to={`/devices/${encodeURIComponent(entry.device.id)}`} className={styles.ref}>
              {entry.device.name}
              {entry.device.lab && <LabPill />}
              {entry.device.deleted && (
                <Badge tone="idle" variant="outline" title="Deleted from the inventory since">
                  gone
                </Badge>
              )}
            </Link>
          )}
          {entry.refs.map((reference) => {
            const to = refLink(reference, entry.device_id);
            const label = reference.title ?? reference.id;
            // `resolved: false` on a kind this view cannot look up means only
            // that: the record is not necessarily gone, and saying so would
            // be an accusation the trail does not support.
            const gone = !reference.resolved && RESOLVABLE_KINDS.has(reference.type);
            const inside = (
              <>
                <span className={styles.refKind}>{reference.type}</span>
                <span className={styles.refText}>{label}</span>
                {gone && (
                  <Badge tone="idle" variant="outline" title="This record no longer exists.">
                    deleted
                  </Badge>
                )}
              </>
            );
            return to ? (
              <Link key={`${reference.type}:${reference.id}`} to={to} className={styles.ref} title={label}>
                {inside}
              </Link>
            ) : (
              <span key={`${reference.type}:${reference.id}`} className={styles.refPlain} title={label}>
                {inside}
              </span>
            );
          })}
          {hasDetail && (
            <button type="button" className={styles.detailToggle} aria-expanded={open} onClick={onToggle}>
              <Icon icon={ChevronDown} size={11} />
              {open ? 'Hide what was recorded' : 'What was recorded'}
            </button>
          )}
          <span className={styles.id}>#{entry.id}</span>
        </p>
        {open && hasDetail && (
          <pre className={styles.detail}>{JSON.stringify(entry.detail, null, 2)}</pre>
        )}
      </div>
    </li>
  );
}
