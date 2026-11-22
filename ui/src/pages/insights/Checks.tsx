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
import { useMemo, useState } from 'react';
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
} from './data';
import {
  CATEGORIES,
  categoryLabel,
  severityLabel,
  severityTone,
  SEVERITIES,
  verdict,
} from './insight';
import type { Scan } from './useScan';

/** What each threshold means and how it reads. The text is the backend's own. */
const THRESHOLDS: {
  key: keyof InsightsThresholds;
  label: string;
  unit: string;
  hint: string;
}[] = [
  {
    key: 'flap_transitions_per_hour',
    label: 'Flapping',
    unit: 'transitions an hour',
    hint: 'Up/down transitions within an hour that count as flapping.',
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
  },
  {
    key: 'history_hours',
    label: 'History read',
    unit: 'hours',
    hint: 'How far back metric and log history is read.',
  },
  {
    key: 'backup_age_days',
    label: 'Backup age',
    unit: 'days',
    hint: 'A backup older than this is "no recent backup".',
  },
  {
    key: 'stale_data_hours',
    label: 'Data goes stale after',
    unit: 'hours',
    hint: 'Collected data older than this is not used for cross-device checks: comparing two ends of a link means comparing them as they were at about the same time.',
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
  },
  {
    key: 'software_min_group',
    label: 'Version majority',
    unit: 'devices of a model',
    hint: 'Devices of one model needed before a majority version means anything.',
  },
  {
    key: 'saved_pair_window_minutes',
    label: 'Running/startup pair',
    unit: 'minutes apart',
    hint: 'How close together the running and startup configurations must have been captured to be compared.',
  },
  {
    key: 'arp_window_minutes',
    label: 'ARP tables',
    unit: 'minutes apart',
    hint: 'How close together two ARP tables must have been read to be compared.',
  },
  {
    key: 'change_window_minutes',
    label: 'Change window',
    unit: 'minutes',
    hint: 'How far from a configuration change an applied change may be and still account for it.',
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

  if (checks.isPending) return <Skeleton shape="block" height={320} />;
  if (checks.error) {
    return (
      <ErrorState
        title="The catalogue could not be read"
        error={checks.error}
        onRetry={() => void checks.refetch()}
      />
    );
  }

  const all = checks.data ?? [];
  const on = all.filter((check) => check.enabled).length;
  const thresholds = settings.data?.thresholds;

  const setEnabled = (check: CheckInfo, enabled: boolean) => {
    save.mutate({ section: 'insights', value: { enabled_checks: { [check.id]: enabled } } });
  };

  return (
    <div className={styles.tab}>
      <Panel
        title="How insights run"
        padding="md"
        actions={
          <span className={styles.count}>
            {formatCount(on)} of {formatCount(all.length)} checks on
          </span>
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
        title="Thresholds"
        padding="md"
        collapsible
        defaultCollapsed
        storageId="insights.thresholds"
        actions={<Badge tone="neutral" variant="outline">{`settings → insights`}</Badge>}
      >
        <p className={styles.lead}>
          <Icon icon={SlidersHorizontal} size={13} /> Every number a check compares against.
          A change takes effect on the next scan.
        </p>
        <div className={styles.thresholds}>
          {THRESHOLDS.map((threshold) => (
            <ThresholdInput
              key={threshold.key}
              label={threshold.label}
              unit={threshold.unit}
              hint={threshold.hint}
              value={thresholds?.[threshold.key]}
              disabled={thresholds === undefined}
              onSave={(value) =>
                save.mutate({
                  section: 'insights',
                  value: { thresholds: { [threshold.key]: value } },
                })
              }
            />
          ))}
        </div>
      </Panel>

      <div className={styles.catalogueHead}>
        <SearchInput
          value={query}
          onChange={setQuery}
          aria-label="Search checks"
          placeholder="Search what a check looks for…"
          className={styles.search}
        />
        <Select
          size="sm"
          className={styles.category}
          aria-label="Category"
          value={category}
          onChange={(event) => setCategory(event.target.value)}
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
                setQuery('');
                setCategory('');
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
                      onChange={(event) => setEnabled(check, event.target.checked)}
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
    </div>
  );
}

interface ThresholdInputProps {
  label: string;
  unit: string;
  hint: string;
  value: number | undefined;
  disabled: boolean;
  onSave: (value: number) => void;
}

/**
 * One threshold. It is saved when the field is left or Enter is pressed, not
 * on every keystroke: a half-typed number is not a setting.
 */
function ThresholdInput({ label, unit, hint, value, disabled, onSave }: ThresholdInputProps) {
  const [text, setText] = useState<string | null>(null);
  const shown = text ?? (value === undefined ? '' : String(value));

  const commit = () => {
    if (text === null) return;
    const next = Number(text);
    setText(null);
    if (Number.isFinite(next) && next >= 0 && next !== value) onSave(next);
  };

  return (
    <Field label={label} hint={hint} className={styles.threshold}>
      <span className={styles.thresholdControl}>
        <Input
          type="number"
          min={0}
          size="sm"
          value={shown}
          disabled={disabled}
          onChange={(event) => setText(event.target.value)}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === 'Enter') commit();
          }}
        />
        <span className={styles.unit}>{unit}</span>
      </span>
    </Field>
  );
}
