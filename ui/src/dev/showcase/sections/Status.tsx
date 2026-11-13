import { Badge } from '@/design/Badge';
import { Card } from '@/design/Card';
import { ProgressBar } from '@/design/ProgressBar';
import { Sparkline } from '@/design/Sparkline';
import { Stat } from '@/design/Stat';
import { StatusDot } from '@/design/StatusDot';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { STATUS_LABEL, type StatusTone, type Tone } from '@/design/tone';
import { formatCount, formatDelta, formatPercent, rateQuantity } from '@/lib/format';
import { Example, Section } from '../parts';
import styles from '../Showcase.module.css';

const STATUSES: StatusTone[] = ['ok', 'warn', 'crit', 'info', 'idle', 'maint'];
const TONES: Tone[] = ['neutral', 'accent', 'ai', ...STATUSES];

const CPU = [38, 41, 37, 44, 52, 49, 58, 63, 55, 61, 67, 64];
const LATENCY = [12, 11, 13, 12, null, null, 15, 14, 19, 26, 31, 38];
const FLAT = [5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5];

export function Status() {
  const throughput = rateQuantity(938_400_000);
  const now = Date.now();

  return (
    <>
      <Section
        title="StatusDot and StatusPill"
        description="Status is never colour alone. Each status has its own silhouette as a dot, and its own icon and word as a pill."
      >
        <Example title="StatusDot: one shape per status">
          {STATUSES.map((tone) => (
            <span key={tone} className={styles.inline} style={{ gap: 6 }}>
              <StatusDot tone={tone} />
              {STATUS_LABEL[tone]}
            </span>
          ))}
        </Example>
        <Example title="Sizes, and a pulse for what is happening now">
          <StatusDot tone="ok" size={8} label="Healthy" />
          <StatusDot tone="warn" size={10} label="Degraded" />
          <StatusDot tone="crit" size={12} label="Critical" />
          <StatusDot tone="crit" size={12} label="Critical, active" pulse />
          <StatusDot tone="ok" size={12} label="Live" pulse />
        </Example>
        <Example title="StatusPill">
          <StatusPill tone="ok">Up</StatusPill>
          <StatusPill tone="warn">Degraded</StatusPill>
          <StatusPill tone="crit">Down</StatusPill>
          <StatusPill tone="info">Syncing</StatusPill>
          <StatusPill tone="idle">Unmonitored</StatusPill>
          <StatusPill tone="maint" />
          <LabPill />
        </Example>
      </Section>

      <Section title="Badge" description="A small label or count. For a state, use StatusPill.">
        <Example title="Soft" span={2}>
          {TONES.map((tone) => (
            <Badge key={tone} tone={tone}>
              {tone}
            </Badge>
          ))}
        </Example>
        <Example title="Outline and counts">
          <Badge variant="outline">v17.9.4</Badge>
          <Badge variant="outline" tone="accent">
            beta
          </Badge>
          <Badge tone="accent">3</Badge>
          <Badge tone="crit">12</Badge>
          <Badge>1.2k</Badge>
        </Example>
      </Section>

      <Section
        title="Stat"
        description="One number that matters. The delta says which way it moved and, by colour and arrow, whether that is good news; the sparkline shows where it has been."
      >
        <Example title="In cards" span={3} layout="fill">
          <div className={styles.statGrid}>
            <Card>
              <Stat
                label="Devices up"
                value={formatCount(1204)}
                unit={`of ${formatCount(1211)}`}
                delta={{ text: formatDelta(3), direction: 'up', sentiment: 'good', period: 'since 09:00' }}
              />
            </Card>
            <Card>
              <Stat
                label="Core throughput"
                value={throughput.value}
                unit={throughput.unit}
                delta={{ text: formatDelta(4.2, formatPercent), direction: 'up', period: 'vs last hour' }}
                trend={<Sparkline values={CPU} label="Core throughput over the last hour, rising" width={96} height={32} area />}
              />
            </Card>
            <Card>
              <Stat
                label="Packet loss"
                value={formatPercent(1.8)}
                delta={{ text: formatDelta(1.3, formatPercent), direction: 'up', sentiment: 'bad', period: 'vs yesterday' }}
                trend={<Sparkline values={LATENCY} label="Packet loss over the last hour, rising, with a gap" width={96} height={32} tone="crit" />}
              />
            </Card>
          </div>
        </Example>
        <Example title="Small, flat, loading" span={3} layout="fill">
          <div className={styles.statGrid}>
            <Stat size="sm" label="Open alerts" value={formatCount(7)} delta={{ text: formatDelta(0), direction: 'flat', period: 'vs yesterday' }} />
            <Stat size="sm" label="Config drift" value={formatCount(null)} unit="devices" />
            <Stat size="sm" label="Pending approvals" value="" loading />
          </div>
        </Example>
      </Section>

      <Section title="Sparkline" description="History in the quiet colour, the latest value marked. No axes: the number sits next to it.">
        <Example title="Tones and shapes" span={2}>
          <Sparkline values={CPU} label="CPU over the last hour: 64% now, between 37% and 67%" />
          <Sparkline values={CPU} label="CPU with a wash" area />
          <Sparkline values={CPU} label="CPU on a fixed 0 to 100 scale" domain={[0, 100]} />
          <Sparkline values={LATENCY} label="Latency with a gap, critical" tone="crit" />
          <Sparkline values={FLAT} label="A flat series" tone="ok" />
          <Sparkline values={[42]} label="A single sample" />
          <Sparkline values={[null, null, null]} label="Latency" />
        </Example>
        <Example title="TimeAgo: keeps itself current, exact time on hover">
          <TimeAgo at={now - 8_000} />
          <TimeAgo at={now - 5 * 60_000} />
          <TimeAgo at={now - 3 * 3_600_000} />
          <TimeAgo at={now - 12 * 86_400_000} />
          <TimeAgo at={null} />
        </Example>
      </Section>

      <Section title="ProgressBar" description="Progress of a task, or a level against a limit. The track is a quiet step of the fill's own colour.">
        <Example title="Progress" layout="column">
          <ProgressBar aria-label="Backing up" value={62} valueText="62%" />
          <ProgressBar aria-label="Assistant working" value={35} tone="ai" size="sm" />
          <ProgressBar aria-label="Working" />
        </Example>
        <Example title="Meter: the tone is the severity" layout="column" span={2}>
          {(
            [
              ['CPU', 34, 'ok'],
              ['Memory', 78, 'warn'],
              ['Disk', 93, 'crit'],
            ] as const
          ).map(([label, value, tone]) => (
            <div key={label} className={styles.inline}>
              <span style={{ width: 64 }}>{label}</span>
              <div style={{ flex: 1 }}>
                <ProgressBar kind="meter" aria-label={label} value={value} tone={tone} valueText={formatPercent(value)} />
              </div>
              <span className="tnum" style={{ width: 40, textAlign: 'right' }}>
                {formatPercent(value)}
              </span>
            </div>
          ))}
        </Example>
      </Section>
    </>
  );
}
