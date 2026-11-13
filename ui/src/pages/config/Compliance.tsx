/**
 * Are the configurations we have the configurations we meant?
 *
 * A policy is a list of rules; a run judges the latest stored configuration
 * of every device in scope. No device is touched, so a run is instant and
 * works offline — and a device with no stored configuration is
 * `not_evaluated`, never a pass. That distinction is the whole reason this
 * screen shows the coverage of a run as prominently as its score.
 *
 * Every finding carries the rule's own rationale and the evidence line it
 * was reached from. Neither is paraphrased here: a hardening rule that is
 * explained wrongly is worse than one that is not explained at all.
 */

import { CircleCheck, FileWarning, ListChecks, Play, ShieldCheck, Wrench } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useNavigate } from 'react-router';
import type {
  ComplianceResultsParams,
  Finding,
  FindingStatus,
  Policy,
  Rule,
  Severity,
} from '@/api/types/config';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Stat } from '@/design/Stat';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, formatPercent, plural } from '@/lib/format';
import styles from './Compliance.module.css';
import { useComplianceRun, usePolicies, useRemediate, useResults, useRuns, useSavePolicy } from './data';
import { Quoted } from './Quoted';
import { RedactedLine } from './Redacted';
import { RuleDetail } from './RuleDetail';
import {
  checkSentence,
  FINDING_LABEL,
  FINDING_SENTENCE,
  findingTone,
  scoreTone,
  SEVERITY_LABEL,
  SEVERITY_ORDER,
  severityTone,
} from './vocab';

export interface ComplianceTabProps {
  /** Opens a device's history, so a finding can be read in context. */
  onOpenDevice: (deviceId: string) => void;
  /** This build can hand findings to the change pipeline. */
  canRemediate: boolean;
}

export function ComplianceTab({ onOpenDevice, canRemediate }: ComplianceTabProps) {
  const policies = usePolicies();
  const runs = useRuns();
  const run = useComplianceRun();
  const [status, setStatus] = useState<FindingStatus | 'all'>('fail');
  const [severity, setSeverity] = useState<Severity | 'all'>('all');
  const [policyId, setPolicyId] = useState<string>('all');
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [rule, setRule] = useState<{ rule: Rule; policy: Policy } | null>(null);

  const params: ComplianceResultsParams = useMemo(
    () => ({
      ...(status === 'all' ? {} : { status }),
      ...(severity === 'all' ? {} : { severity }),
      ...(policyId === 'all' ? {} : { policy_id: policyId }),
      limit: 500,
    }),
    [status, severity, policyId],
  );
  const results = useResults(params);
  // Before the first run the backend answers `not_found` ("no compliance run
  // has been made yet"), which is not an error to show: it is the empty state.
  const neverRun = results.isError && results.error.code === 'not_found';
  const summary = results.data?.summary;
  const remediate = useRemediate();
  const navigate = useNavigate();

  const findings = results.data?.findings ?? [];
  const keyOf = (finding: Finding) => `${finding.policy_id}/${finding.rule_id}/${finding.device_id}`;
  const fixable = findings.filter((finding) => finding.status === 'fail' && finding.remediable);
  const chosen = fixable.filter((finding) => selected.has(keyOf(finding)));

  const everRun = !neverRun && (runs.data?.length ?? 0) > 0;

  if (policies.isError) {
    return (
      <ErrorState
        title="The policies could not be read"
        error={policies.error}
        onRetry={() => void policies.refetch()}
      />
    );
  }

  return (
    <div className={styles.compliance}>
      <div className={styles.head}>
        <div className={styles.stats}>
          <Stat
            label="Score"
            value={summary?.score_pct == null ? '—' : formatPercent(summary.score_pct)}
            loading={results.isPending}
          />
          <Stat label="Failing" value={formatCount(summary?.fail)} loading={results.isPending} />
          <Stat label="Passing" value={formatCount(summary?.pass)} loading={results.isPending} />
          <Stat
            label="Devices covered"
            value={formatCount(summary?.devices)}
            unit={summary ? `of ${formatCount(summary.devices_requested)} asked about` : undefined}
            loading={results.isPending}
          />
        </div>
        <div className={styles.runBar}>
          {summary && (
            <span className={styles.quiet}>
              Last run <TimeAgo at={summary.at} /> · {plural(summary.policies, 'policy', 'policies')},{' '}
              {plural(summary.rules, 'rule')}
            </span>
          )}
          <Button
            variant="primary"
            icon={Play}
            loading={run.pending}
            onClick={() =>
              run.start(
                policyId === 'all' ? {} : { policy_ids: [policyId] },
                policyId === 'all'
                  ? 'Every enabled policy'
                  : (policies.data?.find((policy) => policy.id === policyId)?.name ?? 'That policy'),
              )
            }
          >
            {policyId === 'all' ? 'Run every enabled policy' : 'Run this policy'}
          </Button>
        </div>
      </div>

      {summary && summary.score_pct !== null && (
        <div className={styles.scoreNote} data-tone={scoreTone(summary.score_pct)}>
          <Icon icon={summary.fail === 0 ? CircleCheck : FileWarning} size={14} />
          <span>
            {summary.fail === 0
              ? `Every rule that could be judged passed on all ${plural(summary.devices, 'device')}.`
              : `${plural(summary.fail, 'finding')} across ${plural(summary.devices, 'device')}.`}{' '}
            {summary.not_applicable > 0 &&
              `${formatCount(summary.not_applicable)} rules had nothing to judge. `}
            {summary.devices_without_backup > 0 ? (
              <strong>
                {plural(summary.devices_without_backup, 'device')} had no stored configuration, so{' '}
                {summary.devices_without_backup === 1 ? 'it was' : 'they were'} not judged at all —
                this score is not about {summary.devices_without_backup === 1 ? 'it' : 'them'}.
              </strong>
            ) : (
              'Every device in scope had a stored configuration to judge.'
            )}
          </span>
        </div>
      )}

      <Panel
        title="Policies"
        storageId="config.policies"
        collapsible
        defaultCollapsed
        actions={<Badge>{plural(policies.data?.length ?? 0, 'policy', 'policies')}</Badge>}
      >
        {policies.isPending ? (
          <Skeleton shape="block" height={80} />
        ) : (
          <PolicyList
            policies={policies.data ?? []}
            onRule={(entry) => setRule(entry)}
            onPick={(id) => setPolicyId(id)}
            picked={policyId}
          />
        )}
      </Panel>

      <div className={styles.filters}>
        <Select
          size="sm"
          className={styles.filter}
          aria-label="Finding status"
          value={status}
          onChange={(event) => setStatus(event.target.value as FindingStatus | 'all')}
          options={[
            { value: 'fail', label: 'Failing' },
            { value: 'pass', label: 'Passing' },
            { value: 'not_evaluated', label: 'Not evaluated' },
            { value: 'not_applicable', label: 'Not applicable' },
            { value: 'all', label: 'Every finding' },
          ]}
        />
        <Select
          size="sm"
          className={styles.filter}
          aria-label="Severity"
          value={severity}
          onChange={(event) => setSeverity(event.target.value as Severity | 'all')}
          options={[
            { value: 'all', label: 'Any severity' },
            ...SEVERITY_ORDER.map((entry) => ({ value: entry, label: SEVERITY_LABEL[entry] })),
          ]}
        />
        <Select
          size="sm"
          className={styles.filter}
          aria-label="Policy"
          value={policyId}
          onChange={(event) => setPolicyId(event.target.value)}
          options={[
            { value: 'all', label: 'Every policy' },
            ...(policies.data ?? []).map((policy) => ({ value: policy.id, label: policy.name })),
          ]}
        />
        {canRemediate && chosen.length > 0 && (
          <Button
            variant="secondary"
            icon={Wrench}
            loading={remediate.isPending}
            onClick={() =>
              remediate.mutate(
                {
                  findings: chosen.map((finding) => ({
                    device_id: finding.device_id,
                    rule_id: finding.rule_id,
                    policy_id: finding.policy_id,
                  })),
                },
                {
                  onSuccess: (result) => {
                    setSelected(new Set());
                    toast.success(
                      `A change was drafted for ${plural(result.devices, 'device')}`,
                      {
                        description: `${plural(result.commands, 'line')} would be sent. Nothing has been sent yet.`,
                        action: {
                          label: 'Open the change',
                          onSelect: () => navigate(`/changes/${result.change_id}`),
                        },
                      },
                    );
                  },
                },
              )
            }
          >
            Fix {plural(chosen.length, 'finding')}…
          </Button>
        )}
      </div>

      <Findings
        state={results}
        neverRun={neverRun}
        findings={findings}
        everRun={everRun}
        onRunAll={() => run.start({}, 'Every enabled policy')}
        running={run.pending}
        selected={selected}
        onSelected={setSelected}
        keyOf={keyOf}
        onOpenDevice={onOpenDevice}
        onRule={(finding) => {
          const policy = policies.data?.find((entry) => entry.id === finding.policy_id);
          const found = policy?.rules.find((entry) => entry.id === finding.rule_id);
          if (policy && found) setRule({ rule: found, policy });
        }}
      />

      {rule && (
        <RuleDetail
          rule={rule.rule}
          policy={rule.policy}
          onClose={() => setRule(null)}
        />
      )}
    </div>
  );
}

function PolicyList({
  policies,
  onRule,
  onPick,
  picked,
}: {
  policies: Policy[];
  onRule: (entry: { rule: Rule; policy: Policy }) => void;
  onPick: (policyId: string) => void;
  picked: string;
}) {
  const save = useSavePolicy();
  return (
    <ul className={styles.policies}>
      {policies.map((policy) => (
        <li key={policy.id} className={styles.policy} data-testid="policy">
          <div className={styles.policyHead}>
            <Switch
              checked={policy.enabled}
              onChange={(event) =>
                save.mutate({ policy: { ...policy, enabled: event.target.checked } })
              }
              aria-label={`Include ${policy.name} in a run`}
            />
            <button type="button" className={styles.policyName} onClick={() => onPick(policy.id)}>
              {policy.name}
            </button>
            {policy.builtin && (
              <Tooltip content="Shipped with Nocturne. It can be switched off or duplicated, but not edited.">
                <Badge variant="outline">Built in</Badge>
              </Tooltip>
            )}
            {picked === policy.id && <Badge tone="accent">Filtered to this</Badge>}
            <Badge>{plural(policy.rules.length, 'rule')}</Badge>
            {policy.scope.os.length > 0 && (
              <span className={styles.quiet}>for {policy.scope.os.join(', ')}</span>
            )}
          </div>
          <p className={styles.policyDescription}>
            <Quoted text={policy.description} />
          </p>
          <ul className={styles.rules}>
            {policy.rules.map((rule) => (
              <li key={rule.id}>
                <button
                  type="button"
                  className={styles.rule}
                  onClick={() => onRule({ rule, policy })}
                  data-testid="rule-chip"
                >
                  <Badge tone={severityTone(rule.severity)}>{SEVERITY_LABEL[rule.severity]}</Badge>
                  <span className={styles.ruleTitle}>
                    <Quoted text={rule.title} />
                  </span>
                  <span className={styles.ruleCheck}>
                    <Quoted text={checkSentence(rule.check)} />
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </li>
      ))}
    </ul>
  );
}

function Findings({
  state,
  neverRun,
  findings,
  everRun,
  onRunAll,
  running,
  selected,
  onSelected,
  keyOf,
  onOpenDevice,
  onRule,
}: {
  state: ReturnType<typeof useResults>;
  neverRun: boolean;
  findings: Finding[];
  everRun: boolean;
  onRunAll: () => void;
  running: boolean;
  selected: ReadonlySet<string>;
  onSelected: (selected: Set<string>) => void;
  keyOf: (finding: Finding) => string;
  onOpenDevice: (deviceId: string) => void;
  onRule: (finding: Finding) => void;
}) {
  if (state.isError && !neverRun) {
    return (
      <ErrorState
        title="The findings could not be read"
        error={state.error}
        onRetry={() => void state.refetch()}
      />
    );
  }

  const columns: readonly TableColumn<Finding>[] = [
    {
      id: 'status',
      header: 'Verdict',
      required: true,
      width: 112,
      cell: (finding) => (
        <Tooltip content={FINDING_SENTENCE[finding.status]}>
          <StatusPill tone={findingTone(finding.status)}>{FINDING_LABEL[finding.status]}</StatusPill>
        </Tooltip>
      ),
      sortValue: (finding) => finding.status,
    },
    {
      id: 'severity',
      header: 'Severity',
      width: 112,
      cell: (finding) => (
        <Badge tone={severityTone(finding.severity)}>{SEVERITY_LABEL[finding.severity]}</Badge>
      ),
      sortValue: (finding) => SEVERITY_ORDER.indexOf(finding.severity),
    },
    {
      id: 'device',
      header: 'Device',
      width: 190,
      cell: (finding) => (
        <button type="button" className={styles.deviceLink} onClick={() => onOpenDevice(finding.device_id)}>
          {finding.device_name}
        </button>
      ),
      sortValue: (finding) => finding.device_name.toLowerCase(),
    },
    {
      id: 'rule',
      header: 'Rule',
      cell: (finding) => (
        <button type="button" className={styles.ruleLink} onClick={() => onRule(finding)}>
          <Quoted text={finding.title} />
        </button>
      ),
      sortValue: (finding) => finding.title.toLowerCase(),
    },
    {
      id: 'detail',
      header: 'What the rule found',
      cell: (finding) => (
        <span className={styles.detail} title={finding.detail}>
          <Quoted text={finding.detail} />
        </span>
      ),
      sortValue: (finding) => finding.detail,
    },
    {
      id: 'evidence',
      header: 'Evidence',
      mono: true,
      cell: (finding) =>
        finding.evidence.length === 0 ? (
          finding.missing.length > 0 ? (
            <span className={styles.missing} title={finding.missing.join('\n')}>
              missing: {finding.missing[0]}
              {finding.missing.length > 1 && ` (+${finding.missing.length - 1})`}
            </span>
          ) : (
            ''
          )
        ) : (
          <span className={styles.evidence} title={finding.evidence.map((line) => line.line).join('\n')}>
            <span className={styles.evidenceNo}>{finding.evidence[0]?.line_no}</span>
            <RedactedLine line={finding.evidence[0]?.line ?? ''} />
            {finding.evidence.length > 1 && (
              <span className={styles.quiet}> +{finding.evidence.length - 1} more</span>
            )}
          </span>
        ),
      sortValue: (finding) => finding.evidence[0]?.line ?? '',
    },
    {
      id: 'from',
      header: 'Judged from',
      width: 120,
      defaultHidden: true,
      cell: (finding) =>
        finding.fetched_at ? <TimeAgo at={finding.fetched_at} /> : <span className={styles.quiet}>nothing</span>,
      sortValue: (finding) => finding.fetched_at ?? 0,
    },
  ];

  return (
    <Table
      id="config.findings"
      aria-label="Compliance findings"
      columns={columns}
      rows={findings}
      rowKey={keyOf}
      loading={state.isPending && !neverRun}
      density="compact"
      selection={{ selected, onChange: onSelected }}
      onRowActivate={(finding) => onRule(finding)}
      maxHeight="56vh"
      empty={
        everRun ? (
          <EmptyState
            icon={ShieldCheck}
            title="Nothing matches these filters"
            description="Change the verdict, the severity or the policy above to see other findings from the last run."
            size="compact"
          />
        ) : (
          <EmptyState
            icon={ListChecks}
            title="No policy has been run yet"
            description="A run judges the latest stored configuration of every device in scope. It touches no device, so it is instant and works even when the network is unreachable."
            action={
              <Button variant="primary" icon={Play} loading={running} onClick={onRunAll}>
                Run every enabled policy
              </Button>
            }
            size="compact"
          />
        )
      }
    />
  );
}
