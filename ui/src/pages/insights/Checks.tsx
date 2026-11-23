/**
 * The catalogue: every check Nocturne ships, what it looks for, what it
 * deliberately does not evaluate, whether it is on, and the numbers it
 * compares against.
 *
 * The descriptions and the "not evaluated" sentences are the backend's own
 * (`catalogue.rs`), shown verbatim: a check whose blind spots are not written
 * down is a check nobody can act on. The thresholds are the live `insights`
 * settings section, editable here because a threshold is part of what the
 * check means.
 */

import { ListChecks, ScanSearch, SlidersHorizontal } from 'lucide-react';
import { useMemo, useRef, useState, type KeyboardEvent } from 'react';
import type { CheckCoverage, CheckInfo, InsightsThresholds } from '@/api/types/insights';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Tooltip } from '@/design/Tooltip';
import { EMPTY, formatCount, plural } from '@/lib/format';
import styles from './Checks.module.css';
import {
  useChecks,
  useCoverage,
  useInsightsSettings,
  useSaveSettings,
  useSaveThreshold,
} from './data';
import {
  CATEGORIES,
  categoryLabel,
  readThreshold,
  refusalOf,
  severityLabel,
  severityTone,
  SEVERITIES,
  storedThresholdProblem,
  verdict,
  WHOLE_U32,
  WHOLE_U64,
} from './insight';
import type { Scan } from './useScan';

/**
 * What each threshold means and how it reads. The text is the backend's own.
 * `whole` marks one the engine counts in whole units (`u32`, `u64` in
 * `nocturne-insights/src/settings.rs`) and is the most it can hold; the rest
 * are fractions (`f64`).
 */
const THRESHOLDS: {
  key: keyof InsightsThresholds;
  label: string;
  unit: string;
  hint: string;
  whole?: number;
}[] = [
  {
    key: 'flap_transitions_per_hour',
    label: 'Flapping',
    unit: 'transitions an hour',
    hint: 'Up/down transitions within an hour that count as flapping.',
    whole: WHOLE_U32,
  },
  {
    key: 'interface_error_ppm',
    label: 'Interface errors',
    unit: 'per million packets',
    hint: 'Input errors per million packets, between the last two collections.',
  },
  {
    key: 'interface_error_min_delta',
    label: '…and at least',
    unit: 'new errors',
    hint: 'A rate over a handful of packets is noise, so a minimum number of new errors is needed too.',
    whole: WHOLE_U64,
  },
  {
    key: 'utilisation_pct',
    label: 'Utilisation',
    unit: '%',
    hint: 'Interface utilisation that counts as congested.',
  },
  { key: 'cpu_pct', label: 'CPU', unit: '%', hint: 'Processor load that counts as high.' },
  { key: 'memory_pct', label: 'Memory', unit: '%', hint: 'Memory in use that counts as high.' },
  {
    key: 'temperature_c',
    label: 'Temperature',
    unit: '°C',
    hint: 'Temperature that counts as high.',
  },
  {
    key: 'sustained_minutes',
    label: 'Sustained for',
    unit: 'minutes',
    hint: 'How long a metric must stay over its threshold to count as sustained. One reading is never sustained.',
    whole: WHOLE_U32,
  },
  {
    key: 'history_hours',
    label: 'History read',
    unit: 'hours',
    hint: 'How far back metric and log history is read.',
    whole: WHOLE_U32,
  },
  {
    key: 'backup_age_days',
    label: 'Backup age',
    unit: 'days',
    hint: 'A backup older than this is "no recent backup".',
    whole: WHOLE_U32,
  },
  {
    key: 'stale_data_hours',
    label: 'Data goes stale after',
    unit: 'hours',
    hint: 'Collected data older than this is not used for cross-device checks: comparing two ends of a link means comparing them as they were at about the same time.',
    whole: WHOLE_U32,
  },
  {
    key: 'qos_drop_ps',
    label: 'QoS drops',
    unit: 'packets a second',
    hint: 'Packets a second a QoS class must drop, for the sustained time, before it is reported. A class that drops now and then is doing its job.',
  },
  {
    key: 'mac_flap_min_macs',
    label: 'MAC flapping',
    unit: 'addresses',
    hint: 'Addresses that must move between the same two ports before it counts as a flap rather than somebody moving a laptop.',
    whole: WHOLE_U32,
  },
  {
    key: 'software_min_group',
    label: 'Version majority',
    unit: 'devices of a model',
    hint: 'Devices of one model needed before a majority version means anything.',
    whole: WHOLE_U32,
  },
  {
    key: 'saved_pair_window_minutes',
    label: 'Running/startup pair',
    unit: 'minutes apart',
    hint: 'How close together the running and startup configurations must have been captured to be compared.',
    whole: WHOLE_U32,
  },
  {
    key: 'arp_window_minutes',
    label: 'ARP tables',
    unit: 'minutes apart',
    hint: 'How close together two ARP tables must have been read to be compared.',
    whole: WHOLE_U32,
  },
  {
    key: 'change_window_minutes',
    label: 'Change window',
    unit: 'minutes',
    hint: 'How far from a configuration change an applied change may be and still account for it.',
    whole: WHOLE_U32,
  },
];

/**
 * The thresholds each check compares against, so the number is next to the
 * check it governs. Only the unambiguous ones: `stale_data_hours` applies to
 * every cross-device check and is left in the thresholds panel.
 */
const CHECK_THRESHOLDS: Record<string, (keyof InsightsThresholds)[]> = {
  interface_flapping: ['flap_transitions_per_hour', 'history_hours'],
  interface_errors: ['interface_error_ppm', 'interface_error_min_delta'],
  high_utilisation: ['utilisation_pct', 'sustained_minutes'],
  high_cpu: ['cpu_pct', 'sustained_minutes'],
  high_memory: ['memory_pct', 'sustained_minutes'],
  high_temperature: ['temperature_c', 'sustained_minutes'],
  no_recent_backup: ['backup_age_days'],
  qos_class_dropping: ['qos_drop_ps', 'sustained_minutes', 'history_hours'],
  mac_flapping: ['mac_flap_min_macs'],
  software_inconsistent: ['software_min_group'],
  unsaved_config: ['saved_pair_window_minutes'],
  duplicate_ip: ['arp_window_minutes'],
  config_changed_outside_nocturne: ['change_window_minutes'],
};

const SEVERITY_OPTIONS = SEVERITIES.map((severity) => ({
  value: severity,
  label: severityLabel(severity),
}));

export interface ChecksTabProps {
  scan: Scan;
}

export function ChecksTab({ scan }: ChecksTabProps) {
  const checks = useChecks();
  const settings = useInsightsSettings();
  const coverage = useCoverage();
  const save = useSaveSettings();
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<string>('');

  const lastScan = useMemo(() => {
    const map = new Map<string, CheckCoverage>();
    for (const row of coverage.data?.checks ?? []) map.set(row.check, row);
    return map;
  }, [coverage.data]);

  const rows = useMemo(() => {
    const all = checks.data ?? [];
    const needle = query.trim().toLowerCase();
    return all.filter((check) => {
      if (category !== '' && check.category !== category) return false;
      if (needle === '') return true;
      return [check.id, check.title, check.description, check.not_evaluated, ...check.needs].some(
        (field) => field.toLowerCase().includes(needle),
      );
    });
  }, [checks.data, query, category]);

  const all = checks.data ?? [];
  const on = all.filter((check) => check.enabled).length;
  const thresholds = settings.data?.thresholds;
  // A value the engine cannot read as its threshold (stored before writes
  // were checked, or by hand) can stop the catalogue answering at all. The
  // settings are read on their own, so both panels stay, and the thresholds
  // open, to be where it is put right: nowhere else edits this section.
  const unreadable = THRESHOLDS.some(
    (threshold) => thresholds !== undefined && storedThresholdProblem(thresholds[threshold.key], threshold.whole) !== null,
  );
  // Open for the rest of the visit once it has been: it must not fold away
  // under the field the moment the value in it has been put right.
  const [keptOpen, setKeptOpen] = useState(false);
  if (unreadable && !keptOpen) setKeptOpen(true);

  const setEnabled = (check: CheckInfo, enabled: boolean) => {
    save.mutate({ section: 'insights', value: { enabled_checks: { [check.id]: enabled } } });
  };

  return (
    <div className={styles.tab}>
      <Panel
        title="How insights run"
        padding="md"
        actions={
          checks.data && (
            <span className={styles.count}>
              {formatCount(on)} of {formatCount(all.length)} checks on
            </span>
          )
        }
      >
        {settings.error ? (
          <ErrorState
            size="compact"
            title="The insights settings could not be read"
            error={settings.error}
            onRetry={() => void settings.refetch()}
          />
        ) : (
          <div className={styles.runOptions}>
            <Switch
              checked={settings.data?.auto_scan ?? true}
              disabled={settings.data === undefined}
              onChange={(event) =>
                save.mutate({ section: 'insights', value: { auto_scan: event.target.checked } })
              }
            >
              Scan automatically after a collection or a configuration change
            </Switch>
            <Switch
              checked={settings.data?.include_lab ?? true}
              disabled={settings.data === undefined}
              onChange={(event) =>
                save.mutate({ section: 'insights', value: { include_lab: event.target.checked } })
              }
            >
              Include simulated (lab) devices
            </Switch>
            <Field
              className={styles.alertFrom}
              label="Raise an alert from"
              hint="Open findings at or above this severity call `alerts.raise`, so they also appear on the Monitoring screen. The insight's id is the de-duplication key: one problem is one alert."
            >
              <Select
                size="sm"
                options={SEVERITY_OPTIONS}
                value={settings.data?.raise_alerts_from ?? 'high'}
                disabled={settings.data === undefined}
                onChange={(event) =>
                  save.mutate({
                    section: 'insights',
                    value: { raise_alerts_from: event.target.value },
                  })
                }
              />
            </Field>
          </div>
        )}
      </Panel>

      <Panel
        // Remounted open, past what was remembered, once a stored value has
        // been found wrong: a folded panel would hide the one thing to put right.
        key={keptOpen ? 'open' : 'remembered'}
        title="Thresholds"
        padding="md"
        collapsible
        defaultCollapsed={!keptOpen}
        storageId={keptOpen ? undefined : 'insights.thresholds'}
        actions={<Badge tone="neutral" variant="outline">{`settings → insights`}</Badge>}
      >
        <p className={styles.lead}>
          <Icon icon={SlidersHorizontal} size={13} /> Every number a check compares against.
          A change takes effect on the next scan.
        </p>
        {settings.error ? (
          // Not sixteen empty fields: a number that could not be read is not a blank one.
          <ErrorState
            size="compact"
            title="The thresholds could not be read"
            error={settings.error}
            onRetry={() => void settings.refetch()}
          />
        ) : settings.isPending ? (
          <Skeleton shape="block" height={160} />
        ) : (
          <div className={styles.thresholds}>
            {THRESHOLDS.map((threshold) => (
              <ThresholdInput
                key={threshold.key}
                setting={threshold.key}
                label={threshold.label}
                unit={threshold.unit}
                hint={threshold.hint}
                whole={threshold.whole}
                value={thresholds?.[threshold.key]}
                disabled={thresholds === undefined}
              />
            ))}
          </div>
        )}
      </Panel>

      {checks.isPending ? (
        <Skeleton shape="block" height={320} />
      ) : checks.error ? (
        <ErrorState
          size="compact"
          title="The catalogue could not be read"
          error={checks.error}
          onRetry={() => void checks.refetch()}
        />
      ) : (
        <Catalogue
          rows={rows}
          thresholds={thresholds}
          lastScan={lastScan}
          scan={scan}
          query={query}
          category={category}
          onQuery={setQuery}
          onCategory={setCategory}
          onEnabled={setEnabled}
        />
      )}
    </div>
  );
}

interface CatalogueProps {
  rows: readonly CheckInfo[];
  thresholds: InsightsThresholds | undefined;
  lastScan: ReadonlyMap<string, CheckCoverage>;
  scan: Scan;
  query: string;
  category: string;
  onQuery: (query: string) => void;
  onCategory: (category: string) => void;
  onEnabled: (check: CheckInfo, enabled: boolean) => void;
}

/** The checks themselves, searchable: below the settings, which never depend on it. */
function Catalogue({
  rows,
  thresholds,
  lastScan,
  scan,
  query,
  category,
  onQuery,
  onCategory,
  onEnabled,
}: CatalogueProps) {
  return (
    <>
      <div className={styles.catalogueHead}>
        <SearchInput
          value={query}
          onChange={onQuery}
          aria-label="Search checks"
          placeholder="Search what a check looks for…"
          className={styles.search}
        />
        <Select
          size="sm"
          className={styles.category}
          aria-label="Category"
          value={category}
          onChange={(event) => onCategory(event.target.value)}
          options={[
            { value: '', label: 'Every category' },
            ...CATEGORIES.map((entry) => ({ value: entry, label: categoryLabel(entry) })),
          ]}
        />
        <span className={styles.count}>
          {plural(rows.length, 'check')}
        </span>
      </div>

      {rows.length === 0 ? (
        <EmptyState
          icon={ListChecks}
          title="No check matches"
          description="All of this build's checks are listed here; none of them matches this search."
          action={
            <Button
              variant="secondary"
              onClick={() => {
                onQuery('');
                onCategory('');
              }}
            >
              Clear the search
            </Button>
          }
        />
      ) : (
        <ul className={styles.catalogue}>
          {rows.map((check) => {
            const row = lastScan.get(check.id);
            const words = row ? verdict(row) : null;
            return (
              <li key={check.id}>
                <article className={styles.check} data-off={!check.enabled || undefined}>
                  <header className={styles.checkHead}>
                    <h4 className={styles.checkTitle}>{check.title}</h4>
                    <code className={styles.checkId}>{check.id}</code>
                    <Badge tone="neutral" variant="outline">
                      {categoryLabel(check.category)}
                    </Badge>
                    <Tooltip content="The severity findings from this check are given unless the check itself decides otherwise.">
                      <StatusPill tone={severityTone(check.default_severity)}>
                        {severityLabel(check.default_severity)}
                      </StatusPill>
                    </Tooltip>
                    {words && (
                      <Tooltip content={`In the last scan: ${words.detail}`}>
                        <StatusPill tone={words.tone}>{words.label}</StatusPill>
                      </Tooltip>
                    )}
                    <span className={styles.spacer} />
                    {/* The word as well as the switch: on and off are never
                        told apart by colour alone. It is also the only part a
                        pointer can hit — the drawn track swallows clicks on
                        the input itself. */}
                    <Switch
                      checked={check.enabled}
                      aria-label={`Run the ${check.title} check`}
                      onChange={(event) => onEnabled(check, event.target.checked)}
                    >
                      {check.enabled ? 'On' : 'Off'}
                    </Switch>
                  </header>

                  <p className={styles.description}>{check.description}</p>

                  <p className={styles.notEvaluated}>
                    <span className={styles.inlineLabel}>Not evaluated when:</span>{' '}
                    {check.not_evaluated}
                  </p>

                  <footer className={styles.checkFoot}>
                    <span className={styles.needs}>
                      <span className={styles.inlineLabel}>Reads:</span> {check.needs.join(', ')}
                    </span>
                    {(CHECK_THRESHOLDS[check.id] ?? []).map((key) => {
                      const threshold = THRESHOLDS.find((entry) => entry.key === key);
                      if (!threshold) return null;
                      return (
                        <Tooltip key={key} content={threshold.hint}>
                          <Badge tone="accent" variant="outline">
                            {threshold.label}: {thresholds?.[key] ?? EMPTY} {threshold.unit}
                          </Badge>
                        </Tooltip>
                      );
                    })}
                    <span className={styles.spacer} />
                    <Button
                      size="sm"
                      variant="ghost"
                      icon={ScanSearch}
                      loading={scan.running}
                      onClick={() => scan.start({ checks: [check.id] })}
                      title="Runs this one check over every device, even if it is switched off."
                    >
                      Run just this check
                    </Button>
                  </footer>
                </article>
              </li>
            );
          })}
        </ul>
      )}
    </>
  );
}

interface ThresholdInputProps {
  setting: keyof InsightsThresholds;
  label: string;
  unit: string;
  hint: string;
  /** For a threshold counted in whole units: the most it can hold. */
  whole?: number;
  /** As stored: `settings.get` hands back what is there, right or not. */
  value: number | undefined;
  disabled: boolean;
}

/**
 * One threshold. It is saved when the field is left or Enter is pressed, not
 * on every keystroke: a half-typed number is not a setting. What is wrong
 * with it — as typed, as the engine answered, or as it is stored — is said
 * beside it, and what was typed stays to be corrected; Escape puts the
 * stored value back.
 */
function ThresholdInput({ setting, label, unit, hint, whole, value, disabled }: ThresholdInputProps) {
  const save = useSaveThreshold();
  const [text, setText] = useState<string | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  /** The text last sent, so leaving the field again does not send it twice. */
  const sent = useRef<string | null>(null);
  const shown = text ?? (value === undefined ? '' : String(value));
  const stored = text === null && value !== undefined ? storedThresholdProblem(value, whole) : null;

  const revert = () => {
    setText(null);
    setRefusal(null);
    sent.current = null;
  };

  const commit = (input: HTMLInputElement) => {
    if (text === null || text === sent.current) return;
    const entry = readThreshold(text, whole, input.validity.badInput);
    if (entry.kind === 'unchanged' || (entry.kind === 'value' && entry.value === value)) {
      revert();
      return;
    }
    if (entry.kind === 'refused') {
      setRefusal(entry.reason);
      return;
    }
    const sending = text;
    sent.current = sending;
    save.mutate(
      { section: 'insights', value: { thresholds: { [setting]: entry.value } } },
      {
        // Only what was sent is settled by its answer: anything typed while
        // it was on its way stays in the field.
        onSuccess: () => {
          setText((current) => (current === sending ? null : current));
          if (sent.current === sending) {
            sent.current = null;
            setRefusal(null);
          }
        },
        onError: (error) => {
          if (sent.current === sending) setRefusal(refusalOf(error, setting));
        },
      },
    );
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Enter') commit(event.currentTarget);
    else if (event.key === 'Escape' && text !== null) {
      // This Escape is the field's: nothing around it should act on it too.
      event.preventDefault();
      revert();
    }
  };

  return (
    <Field label={label} hint={hint} error={refusal ?? stored ?? undefined} className={styles.threshold}>
      <span className={styles.thresholdControl}>
        <Input
          type="number"
          min={0}
          max={whole}
          step={whole === undefined ? 'any' : 1}
          inputMode={whole === undefined ? 'decimal' : 'numeric'}
          size="sm"
          value={shown}
          disabled={disabled}
          aria-busy={save.isPending || undefined}
          onChange={(event) => {
            setText(event.target.value);
            setRefusal(null);
            sent.current = null;
          }}
          onBlur={(event) => commit(event.currentTarget)}
          onKeyDown={onKeyDown}
        />
        <span className={styles.unit}>{unit}</span>
      </span>
    </Field>
  );
}
