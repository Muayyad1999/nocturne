/**
 * One rule, in full: what it looks for, why it matters, what it would send
 * to fix it, and — because a rule is only as good as its behaviour on a real
 * configuration — what it concludes about a device you choose.
 *
 * The rationale is the rule's own text, rendered as it was written. The
 * built-in rules say both what they check and what they cannot tell from the
 * configuration text alone, which is exactly the part a screen must not
 * summarise away.
 */

import { FlaskConical } from 'lucide-react';
import { useState } from 'react';
import type { Policy, Rule } from '@/api/types/config';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Drawer } from '@/design/Drawer';
import { Spinner } from '@/design/Spinner';
import { StatusPill } from '@/design/StatusPill';
import { useCoverage, useTestRule } from './data';
import { DevicePicker } from './DevicePicker';
import { Quoted } from './Quoted';
import { RedactedLine } from './Redacted';
import styles from './RuleDetail.module.css';
import { checkSentence, FINDING_LABEL, FINDING_SENTENCE, findingTone, SEVERITY_LABEL, severityTone } from './vocab';

export interface RuleDetailProps {
  rule: Rule;
  policy: Policy;
  onClose: () => void;
}

export function RuleDetail({ rule, policy, onClose }: RuleDetailProps) {
  const coverage = useCoverage();
  const [deviceId, setDeviceId] = useState<string | null>(null);
  const test = useTestRule();

  return (
    <Drawer open onClose={onClose} title={rule.title} description={policy.name} size="lg">
      <div className={styles.detail}>
        <div className={styles.chips}>
          <Badge tone={severityTone(rule.severity)}>{SEVERITY_LABEL[rule.severity]}</Badge>
          <Badge variant="outline">{rule.id}</Badge>
          {policy.builtin && <Badge variant="outline">Built in</Badge>}
        </div>

        <section>
          <h3>What it looks for</h3>
          <p className={styles.sentence}>
            <Quoted text={checkSentence(rule.check)} />
          </p>
          <CodeBlock
            code={JSON.stringify(rule.check, null, 2)}
            title="The check, as the engine holds it"
            wrap
          />
        </section>

        <section>
          <h3>Why it matters</h3>
          <p className={styles.rationale}>
            <Quoted text={rule.rationale} />
          </p>
        </section>

        {rule.remediation && rule.remediation.length > 0 ? (
          <section>
            <h3>What would be sent to fix it</h3>
            <p className={styles.sentence}>
              These lines go to the change pipeline, which plans them, shows them and waits for an
              approval. Nothing here reaches a device.
            </p>
            <CodeBlock code={rule.remediation.join('\n')} lineNumbers title="Remediation lines" />
          </section>
        ) : (
          <section>
            <h3>Fixing it</h3>
            <p className={styles.sentence}>
              This rule carries no remediation lines, so Nocturne will not offer to draft a change for
              it: what the right fix is depends on the site.
            </p>
          </section>
        )}

        <section>
          <h3>Try it on a device</h3>
          <p className={styles.sentence}>
            The rule is applied to that device’s latest stored configuration, here and now. Nothing is
            stored and no device is touched.
          </p>
          <div className={styles.tryRow}>
            <DevicePicker
              coverage={coverage.data}
              value={deviceId}
              onChange={setDeviceId}
              label="Device to test the rule on"
            />
            <Button
              icon={FlaskConical}
              disabled={deviceId === null}
              loading={test.isPending}
              onClick={() => deviceId && test.mutate({ rule, device_id: deviceId })}
            >
              Test
            </Button>
          </div>
          {test.isPending && (
            <p className={styles.sentence}>
              <Spinner size={12} /> Judging…
            </p>
          )}
          {test.error && <p className={styles.failed}>{test.error.message}</p>}
          {test.data && (
            <div className={styles.result} data-testid="rule-test">
              <div className={styles.resultHead}>
                <StatusPill tone={findingTone(test.data.finding.status)}>
                  {FINDING_LABEL[test.data.finding.status]}
                </StatusPill>
                <span className={styles.quiet}>read as {test.data.syntax}</span>
              </div>
              <p className={styles.sentence}>
                <Quoted text={test.data.finding.detail} />
              </p>
              <p className={styles.quiet}>{FINDING_SENTENCE[test.data.finding.status]}</p>
              {test.data.finding.evidence.length > 0 && (
                <ul className={styles.evidence}>
                  {test.data.finding.evidence.map((line) => (
                    <li key={line.line_no}>
                      <span className={styles.lineNo}>{line.line_no}</span>
                      <RedactedLine line={line.line} />
                    </li>
                  ))}
                </ul>
              )}
              {test.data.finding.missing.length > 0 && (
                <ul className={styles.evidence} data-missing>
                  {test.data.finding.missing.map((line) => (
                    <li key={line}>
                      <span className={styles.lineNo}>missing</span>
                      {line}
                    </li>
                  ))}
                </ul>
              )}
              {test.data.fix.length > 0 && (
                <CodeBlock code={test.data.fix.join('\n')} title="What would be sent on this device" />
              )}
            </div>
          )}
        </section>
      </div>
    </Drawer>
  );
}
