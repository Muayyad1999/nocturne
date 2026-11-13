/**
 * "Who else has this line?"
 *
 * One field, every device's latest stored configuration, the matching lines
 * in their context and the section they sit in. This is the thing an
 * engineer types into ten times a day, so: the field has focus when the tab
 * opens, `/` brings it back, the query is debounced rather than requiring
 * Enter, and the query lives in the URL so a result can be pasted into a
 * ticket.
 *
 * It also says what it did *not* look at. A search that quietly skipped the
 * eleven devices with no stored configuration would be worse than useless on
 * a bad day: "it is not configured anywhere" and "nobody has backed those
 * eleven up" are different answers.
 */

import { CornerDownRight, Regex as RegexIcon, Search, SearchX, Type } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type { ConfigKind, SearchMatch } from '@/api/types/config';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { LabPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, plural } from '@/lib/format';
import { useMissing, useSearch } from './data';
import { splitMatches } from './find';
import styles from './NetworkSearch.module.css';
import { RedactedLine } from './Redacted';
import { sectionLabel } from './vocab';

/** Long enough that typing is not a request per keystroke, short enough to feel live. */
const DEBOUNCE_MS = 180;

export interface NetworkSearchProps {
  query: string;
  onQuery: (query: string) => void;
  regex: boolean;
  onRegex: (regex: boolean) => void;
  caseSensitive: boolean;
  onCaseSensitive: (value: boolean) => void;
  kind: ConfigKind;
  onKind: (kind: ConfigKind) => void;
  /** Opens one device's history from a result. */
  onOpen: (deviceId: string) => void;
  /** Registered by the screen so `/` comes back here. */
  focusRef: React.RefObject<HTMLInputElement | null>;
}

export function NetworkSearchTab({
  query,
  onQuery,
  regex,
  onRegex,
  caseSensitive,
  onCaseSensitive,
  kind,
  onKind,
  onOpen,
  focusRef,
}: NetworkSearchProps) {
  const [typed, setTyped] = useState(query);
  const [sent, setSent] = useState(query);
  /** Also ask which devices do *not* have the line. */
  const [invert, setInvert] = useState(false);

  // The field is the fast thing; the request follows it.
  useEffect(() => {
    const timer = setTimeout(() => setSent(typed.trim()), DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [typed]);

  useEffect(() => {
    if (sent !== query) onQuery(sent);
  }, [sent, query, onQuery]);

  useEffect(() => {
    focusRef.current?.focus();
  }, [focusRef]);

  const enabled = sent.length > 0;
  const params = { query: sent, regex, case: caseSensitive, context: 1, kind };
  const search = useSearch(params, enabled && !invert);
  const missing = useMissing({ query: sent, regex, case: caseSensitive, kind }, enabled && invert);

  const grouped = useMemo(() => groupByDevice(search.data?.matches ?? []), [search.data?.matches]);

  return (
    <div className={styles.search}>
      <div className={styles.field}>
        <SearchInput
          ref={focusRef}
          value={typed}
          onChange={setTyped}
          aria-label="Search every device's configuration"
          placeholder="A line, or part of one: snmp-server community, ip helper-address 10.1.1.1, vlan 30…"
          className={styles.input}
          autoComplete="off"
        />
        <Button
          size="sm"
          variant={regex ? 'primary' : 'ghost'}
          icon={RegexIcon}
          aria-pressed={regex}
          onClick={() => onRegex(!regex)}
        >
          Regex
        </Button>
        <Button
          size="sm"
          variant={caseSensitive ? 'primary' : 'ghost'}
          icon={Type}
          aria-pressed={caseSensitive}
          onClick={() => onCaseSensitive(!caseSensitive)}
        >
          Match case
        </Button>
        <Select
          size="sm"
          className={styles.kind}
          aria-label="Which configuration to search"
          value={kind}
          onChange={(event) => onKind(event.target.value as ConfigKind)}
          options={[
            { value: 'running', label: 'Running' },
            { value: 'startup', label: 'Startup' },
          ]}
        />
        <Button
          size="sm"
          variant={invert ? 'primary' : 'ghost'}
          icon={SearchX}
          aria-pressed={invert}
          onClick={() => setInvert(!invert)}
        >
          Who hasn’t got it
        </Button>
      </div>

      {!enabled ? (
        <EmptyState
          icon={Search}
          title="Search every configuration at once"
          description="Type a line and Nocturne looks through the latest stored configuration of every device — with the lines around each hit and the section they are in. Secrets are always hidden in these results."
        />
      ) : invert ? (
        <MissingResults state={missing} query={sent} onOpen={onOpen} />
      ) : search.isError ? (
        <ErrorState
          title="That search could not be run"
          error={search.error}
          onRetry={() => void search.refetch()}
        />
      ) : search.isPending ? (
        <div className={styles.loading}>
          <Skeleton shape="block" height={24} />
          <Skeleton shape="block" height={72} />
          <Skeleton shape="block" height={72} />
        </div>
      ) : search.data ? (
        <>
          <Coverage
            searched={search.data.devices_searched}
            matched={search.data.devices_matched}
            without={search.data.devices_without_backup}
            hits={search.data.matches.length}
            truncated={search.data.truncated}
            note={search.data.note}
            stale={search.isFetching}
          />
          {search.data.matches.length === 0 ? (
            <EmptyState
              icon={SearchX}
              title={`No device has a line matching “${sent}”`}
              description={
                search.data.devices_without_backup > 0
                  ? `Of ${plural(search.data.devices_searched + search.data.devices_without_backup, 'device')}, ${formatCount(search.data.devices_without_backup)} had no stored configuration to look in — so this is an answer about ${plural(search.data.devices_searched, 'device')}, not about the network.`
                  : `All ${plural(search.data.devices_searched, 'device')} with a stored configuration were looked at.`
              }
              size="compact"
            />
          ) : (
            <ul className={styles.results} aria-label="Search results">
              {grouped.map((group) => (
                <li key={group.device_id} className={styles.group} data-testid="search-group">
                  <div className={styles.groupHead}>
                    <button type="button" className={styles.device} onClick={() => onOpen(group.device_id)}>
                      {group.name}
                    </button>
                    {group.lab && <LabPill />}
                    <Badge tone="accent">{plural(group.matches.length, 'line')}</Badge>
                    <span className={styles.quiet}>version {group.version_id}</span>
                  </div>
                  <ul className={styles.hits}>
                    {group.matches.map((match) => (
                      <li key={`${match.line_no}`} className={styles.hit} data-testid="search-hit">
                        <div className={styles.section}>
                          <Icon icon={CornerDownRight} size={11} />
                          {sectionLabel(match.section)}
                        </div>
                        <pre className={styles.lines}>
                          {match.before.map((line, index) => (
                            <span key={`b${index}`} className={styles.context}>
                              <span className={styles.lineNo}>
                                {match.line_no - match.before.length + index}
                              </span>
                              <RedactedLine line={line} />
                            </span>
                          ))}
                          <span className={styles.match}>
                            <span className={styles.lineNo}>{match.line_no}</span>
                            <span>
                              {splitMatches(match.line, sent, { regex, caseSensitive }).map((part, index) =>
                                part.match ? (
                                  <mark key={index} className={styles.mark}>
                                    {part.text}
                                  </mark>
                                ) : (
                                  <RedactedLine key={index} line={part.text} />
                                ),
                              )}
                            </span>
                          </span>
                          {match.after.map((line, index) => (
                            <span key={`a${index}`} className={styles.context}>
                              <span className={styles.lineNo}>{match.line_no + index + 1}</span>
                              <RedactedLine line={line} />
                            </span>
                          ))}
                        </pre>
                      </li>
                    ))}
                  </ul>
                </li>
              ))}
            </ul>
          )}
        </>
      ) : null}
    </div>
  );
}

interface Group {
  device_id: string;
  name: string;
  lab: boolean;
  version_id: number;
  matches: SearchMatch[];
}

/** The backend returns matches in device order; this keeps that order. */
function groupByDevice(matches: readonly SearchMatch[]): Group[] {
  const groups: Group[] = [];
  for (const match of matches) {
    const last = groups.find((group) => group.device_id === match.device_id);
    if (last) last.matches.push(match);
    else {
      groups.push({
        device_id: match.device_id,
        name: match.name,
        lab: match.lab,
        version_id: match.version_id,
        matches: [match],
      });
    }
  }
  return groups;
}

/** How much of the network this answer is actually about. */
function Coverage({
  searched,
  matched,
  without,
  hits,
  truncated,
  note,
  stale,
}: {
  searched: number;
  matched: number;
  without: number;
  hits: number;
  truncated: boolean;
  note?: string;
  stale: boolean;
}) {
  return (
    <div className={styles.coverage} data-busy={stale || undefined} data-testid="search-coverage">
      <strong>{plural(hits, 'line')}</strong>
      <span>
        on <strong>{plural(matched, 'device')}</strong>
      </span>
      <span className={styles.quiet}>
        of {formatCount(searched)} searched
        {without > 0 && (
          <Tooltip content="These devices have no stored configuration of this kind, so whether they have the line is unknown. Back them up to find out.">
            <span className={styles.gap}>
              {' · '}
              {formatCount(without)} had nothing to search
            </span>
          </Tooltip>
        )}
      </span>
      {truncated && (
        <Badge tone="warn">Stopped at the first {formatCount(hits)} lines — narrow the search</Badge>
      )}
      {note && <Badge tone="info">{note}</Badge>}
    </div>
  );
}

function MissingResults({
  state,
  query,
  onOpen,
}: {
  state: ReturnType<typeof useMissing>;
  query: string;
  onOpen: (deviceId: string) => void;
}) {
  if (state.isError) {
    return (
      <ErrorState
        title="That search could not be run"
        error={state.error}
        onRetry={() => void state.refetch()}
      />
    );
  }
  if (state.isPending || !state.data) {
    return <Skeleton shape="block" height={160} />;
  }
  const { devices, devices_searched, without_backup } = state.data;
  return (
    <>
      <div className={styles.coverage}>
        <strong>{plural(devices.length, 'device')}</strong>
        <span>
          have no line matching <code>{query}</code>
        </span>
        <span className={styles.quiet}>of {formatCount(devices_searched)} searched</span>
      </div>
      {devices.length === 0 ? (
        <EmptyState
          icon={Search}
          title="Every device searched has it"
          description={`All ${plural(devices_searched, 'device')} with a stored configuration contain a matching line.`}
          size="compact"
        />
      ) : (
        <ul className={styles.plain} aria-label="Devices without a match">
          {devices.map((device) => (
            <li key={device.device_id}>
              <button type="button" className={styles.device} onClick={() => onOpen(device.device_id)}>
                {device.name}
              </button>
              {device.lab && <LabPill />}
              <span className={styles.quiet}>
                judged from version {device.version_id} · <TimeAgo at={device.fetched_at} />
              </span>
            </li>
          ))}
        </ul>
      )}
      {without_backup.length > 0 && (
        <p className={styles.unknown} data-testid="without-backup">
          {plural(without_backup.length, 'device')} could not be judged at all, because nothing has ever
          been captured from them: {without_backup.map((device) => device.name).join(', ')}.
        </p>
      )}
    </>
  );
}
