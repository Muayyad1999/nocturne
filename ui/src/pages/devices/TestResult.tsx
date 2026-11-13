/**
 * What "Test connection" came back with, in words.
 *
 * This is where a user decides whether to trust Nocturne with their network,
 * so the answer is read like a sentence and not like JSON: how far it got,
 * what answered, what Nocturne decided the device is, and anything that
 * deserves a second look (legacy SSH algorithms, an unknown host key).
 */

import { CircleCheck, CircleX, TriangleAlert } from 'lucide-react';
import { Fragment } from 'react';
import { Icon } from '@/design/Icon';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { Skeleton } from '@/design/Skeleton';
import { formatDuration } from '@/lib/format';
import type { ConnectionTest, TestStage } from '@/api/types/core';
import styles from './TestResult.module.css';

/** The stages a test goes through, in order. */
const STAGES: { id: TestStage | 'ready'; label: string }[] = [
  { id: 'connect', label: 'Reach the device' },
  { id: 'login', label: 'Log in' },
  { id: 'ready', label: 'Read the prompt' },
];

function stageState(stage: TestStage | 'ready', result: ConnectionTest): 'done' | 'failed' | 'todo' {
  if (result.ok) return 'done';
  const failedIndex = STAGES.findIndex((entry) => entry.id === result.failed_at);
  const index = STAGES.findIndex((entry) => entry.id === stage);
  if (failedIndex < 0) return 'todo';
  if (index < failedIndex) return 'done';
  return index === failedIndex ? 'failed' : 'todo';
}

/** What a failure at each stage usually means, in one sentence. */
const STAGE_HINT: Record<TestStage, string> = {
  connect:
    'Nocturne never reached the device: check the address and port, that it is powered up, and that a firewall is not in the way.',
  login:
    'The device answered but the session never got to a usable prompt: check the credential, and whether the device type is right.',
};

export interface TestResultProps {
  result: ConnectionTest;
  /** How the device type is spelled for a person (`cisco_ios` → "Cisco IOS"). */
  profileName?: (id: string) => string;
}

/** The outcome of one `devices.test` call. */
export function TestResult({ result, profileName }: TestResultProps) {
  const info = result.info;
  const type = result.profile ? (profileName?.(result.profile) ?? result.profile) : null;

  const facts: KeyValueItem[] = [];
  if (result.hostname) facts.push({ label: 'Calls itself', value: result.hostname, mono: true });
  if (type) {
    facts.push({
      label: 'Device type',
      value: result.profile_detected ? `${type} (detected)` : type,
    });
  }
  if (info?.peer) facts.push({ label: 'Answered at', value: info.peer, mono: true });
  if (info?.via?.length) facts.push({ label: 'Through', value: info.via.join(' → '), mono: true });
  if (info?.host_key_fingerprint) {
    facts.push({
      label: 'Host key',
      value: `${info.host_key_algorithm ?? ''} ${info.host_key_fingerprint}`.trim(),
      mono: true,
    });
  }

  const crypto: KeyValueItem[] = [
    { label: 'Key exchange', value: info?.kex ?? null, mono: true },
    { label: 'Cipher', value: info?.cipher ?? null, mono: true },
    { label: 'MAC', value: info?.mac ?? null, mono: true },
    { label: 'Identification', value: info?.server_banner?.split('\n')[0] ?? null, mono: true },
  ];
  const hasCrypto = crypto.some((item) => item.value != null);

  return (
    <div className={styles.result} data-tone={result.ok ? 'ok' : 'crit'} role="status">
      <div className={styles.head}>
        <Icon icon={result.ok ? CircleCheck : CircleX} size={16} className={styles.verdictIcon} />
        <h4 className={styles.headline}>
          {result.ok ? 'Connected and logged in' : 'Could not connect'}
        </h4>
        <span className={styles.elapsed}>{formatDuration(result.elapsed_ms)}</span>
      </div>

      <ol className={styles.stages}>
        {STAGES.map((stage, index) => (
          <Fragment key={stage.id}>
            {index > 0 && (
              <li className={styles.arrow} aria-hidden="true">
                →
              </li>
            )}
            <li className={styles.stage} data-state={stageState(stage.id, result)}>
              {stage.label}
            </li>
          </Fragment>
        ))}
      </ol>

      {!result.ok && result.error && (
        <>
          <p className={styles.message}>{result.error.message}</p>
          {result.failed_at && <p className={styles.hint}>{STAGE_HINT[result.failed_at]}</p>}
        </>
      )}

      {result.warnings.length > 0 && (
        <ul className={styles.warnings}>
          {result.warnings.map((warning) => (
            <li key={warning} className={styles.warning}>
              <Icon icon={TriangleAlert} size={14} className={styles.warningIcon} />
              <span>{warning}</span>
            </li>
          ))}
        </ul>
      )}

      {facts.length > 0 && <KeyValue className={styles.facts} items={facts} />}

      {result.detection && (
        <p className={styles.hint}>
          Recognised from the {result.detection.method === 'snmp' ? 'SNMP description' : result.detection.method === 'banner' ? 'login banner' : 'answer to a probe command'}:{' '}
          <code className="mono">{result.detection.evidence.trim().slice(0, 160)}</code>
        </p>
      )}

      {hasCrypto && (
        <details className={styles.detail}>
          <summary className={styles.summary}>What was negotiated</summary>
          <div className={styles.detailBody}>
            <KeyValue items={crypto} />
          </div>
        </details>
      )}
    </div>
  );
}

/** The same box while the test is still running, so the layout does not jump. */
export function TestResultPending() {
  return (
    <div className={styles.result} aria-busy="true">
      <div className={styles.head}>
        <Skeleton width={16} height={16} shape="circle" />
        <Skeleton width={180} height={14} />
      </div>
      <Skeleton width="80%" height={12} />
      <Skeleton width="55%" height={12} />
    </div>
  );
}
