/**
 * The drawer's right side: one tool, framed by what kind of thing it is.
 *
 * The frame is the screen's main promise to the reader. A subnet calculator
 * and a port scan must not look alike, so the header carries the family's
 * colour, its word, and one sentence saying what will happen if you use it —
 * *before* you use it:
 *
 * * **Arithmetic** (quiet, no colour): nothing leaves the machine.
 * * **A probe** (`--info`): packets leave the machine the engine runs on, and
 *   it is recorded in the history.
 * * **Something standing** (`--accent`): it runs on a device, or it keeps
 *   running after this screen is closed.
 *
 * The body is chosen by kind, and every kind ends up in one of the five
 * behaviours: answer as you type, answer when asked, run as a job, hold a
 * standing server, or list the record.
 */

import type { DeviceRow } from '@/api/types/core';
import type { DeviceProbeResult } from '@/api/types/toolbox';
import { Badge } from '@/design/Badge';
import { LabPill } from '@/design/StatusPill';
import { Tooltip } from '@/design/Tooltip';
import { AskedAnswer, LiveAnswer } from './Answer';
import { FAMILY, KIND_BADGE, KIND_PROMISE, type Panel } from './catalogue';
import { useForm, type Form } from './data';
import { HistoryPanel } from './History';
import { JobProbe } from './JobProbe';
import { Reference } from './Reference';
import { Decode, Encode, Identify, KeyTool, PasswordTool } from './Secrets';
import { SyslogPanel, TftpPanel } from './Servers';
import { specFor } from './data';
import type { Spec } from './specs';
import styles from './Tools.module.css';
import { Absence } from './Unavailable';

export interface ToolPanelProps {
  panel: Panel;
  devices: readonly DeviceRow[] | undefined;
}

export function ToolPanel({ panel, devices }: ToolPanelProps) {
  const { tool, kind } = panel;
  const spec = specFor(tool.id);
  const form = useForm(tool.id, spec);

  return (
    <article className={styles.panel} data-family={FAMILY[kind]} aria-labelledby="tool-title">
      <header className={styles.panelHead}>
        <div className={styles.panelTitleRow}>
          <h2 id="tool-title" className={styles.panelTitle}>
            {tool.title}
          </h2>
          <Tooltip content={`The RPC method behind this tool: ${tool.id}`}>
            <Badge tone="neutral" variant="outline" className={styles.panelMethod}>
              {tool.id}
            </Badge>
          </Tooltip>
          <span className={styles.panelKind}>{KIND_BADGE[kind]}</span>
        </div>
        <p className={styles.panelWhat}>{tool.description}</p>
        <p className={styles.panelPromise}>{KIND_PROMISE[kind]}</p>
        {panel.uses.length > 0 && (
          <p className={styles.panelUses}>
            Also uses <code>{panel.uses.join(', ')}</code>
          </p>
        )}
      </header>

      <div className={styles.panelBody}>
        <Body panel={panel} spec={spec} form={form} devices={devices} />
      </div>

      {spec && spec.fields.length > 0 && !form.untouched && (
        <footer className={styles.panelFoot}>
          <button type="button" className={styles.clear} onClick={form.clear}>
            Clear this form
          </button>
          {spec.secret ? (
            <span className={styles.footNote}>
              The inputs of this tool are never remembered: nothing here is written to this
              computer.
            </span>
          ) : (
            <span className={styles.footNote}>What is typed here is remembered on this computer only.</span>
          )}
        </footer>
      )}
    </article>
  );
}

interface BodyProps {
  panel: Panel;
  spec: Spec | undefined;
  form: Form;
  devices: readonly DeviceRow[] | undefined;
}

function Body({ panel, spec, form, devices }: BodyProps) {
  const { tool, kind } = panel;

  if (!spec || kind === 'unformed') {
    return (
      <Absence
        reason="no_form"
        message={`${tool.id} is in this build and the assistant can use it, but this screen has no form for it yet. Nothing is hidden: the catalogue lists it because the engine answers it.`}
      />
    );
  }

  switch (tool.id) {
    case 'tools.secret.identify':
      return <Identify spec={spec} form={form} />;
    case 'tools.secret.decode':
      return <Decode spec={spec} form={form} />;
    case 'tools.secret.encode':
      return <Encode spec={spec} form={form} />;
    case 'tools.password':
      return <PasswordTool spec={spec} form={form} />;
    case 'tools.keys.generate':
      return <KeyTool spec={spec} form={form} />;
    case 'tools.reference':
      return <Reference form={form} />;
    case 'probe.history':
      return <HistoryPanel form={form} />;
    case 'tftp.start':
      return <TftpPanel spec={spec} form={form} />;
    case 'probe.syslog.start':
      return <SyslogPanel spec={spec} form={form} />;
    default:
      break;
  }

  if (kind === 'job') return <JobProbe id={tool.id} spec={spec} form={form} devices={devices} />;

  if (kind === 'probe' || kind === 'device') {
    return (
      <AskedAnswer
        id={tool.id}
        spec={spec}
        form={form}
        devices={devices}
        probe
        what={`the ${tool.title}`}
        footer={
          kind === 'device'
            ? (result) => <LabNote devices={devices} result={result as DeviceProbeResult} />
            : undefined
        }
      />
    );
  }

  if (kind === 'generator') return <AskedAnswer id={tool.id} spec={spec} form={form} />;

  return <LiveAnswer id={tool.id} spec={spec} form={form} />;
}

/**
 * A simulated device is labelled wherever it appears, this screen included:
 * a ping that came back in 0.1 ms from a router that does not exist should
 * never be mistaken for one that does.
 */
function LabNote({
  devices,
  result,
}: {
  devices: readonly DeviceRow[] | undefined;
  result: DeviceProbeResult;
}) {
  const device = devices?.find((entry) => entry.id === result.device_id);
  if (!device?.lab) return null;
  return (
    <p className={styles.labNote}>
      <LabPill /> {result.device_name} is a simulated device. What it printed came from the
      simulator, not from hardware.
    </p>
  );
}
