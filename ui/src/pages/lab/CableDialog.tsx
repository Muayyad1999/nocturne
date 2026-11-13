/**
 * Cabling with the ports chosen by hand, for when the first free one is not
 * the one you want. The quick way — click one device, click another — is on
 * the canvas; this is the long way, and it is the only place a port is named.
 *
 * Only free ports are offered, because a cabled one is not a choice: the
 * simulator refuses it and saying so after the fact would be a worse way to
 * learn it.
 */

import { useMemo, useState } from 'react';
import type { CableKind, LabView, NodeView } from '@/api/types/lab';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { consoleRefusal, defaultInterface } from './cabling';
import { freeInterfaces } from './model';
import styles from './CableDialog.module.css';

export interface CableDialogProps {
  open: boolean;
  onClose: () => void;
  lab: LabView;
  /** The node the dialog opens on. */
  from: NodeView | null;
  onConnect: (params: {
    a: { node: string; interface: string };
    b: { node: string; interface: string };
    cable: CableKind;
  }) => void;
  busy: boolean;
  /** What the backend said about the last attempt, if it refused. */
  error: string | null;
}

const CABLES: { value: CableKind; label: string }[] = [
  { value: 'auto', label: 'Automatic' },
  { value: 'copper', label: 'Copper' },
  { value: 'fiber', label: 'Fibre' },
  { value: 'console', label: 'Console' },
];

export function CableDialog({ open, onClose, lab, from, onConnect, busy, error }: CableDialogProps) {
  const nodes = lab.snapshot.nodes;
  const [cable, setCable] = useState<CableKind>('auto');
  const [toName, setToName] = useState('');
  const [aPort, setAPort] = useState('');
  const [bPort, setBPort] = useState('');

  const others = useMemo(
    () => nodes.filter((node) => node.name !== from?.name && freeInterfaces(node).length > 0),
    [nodes, from],
  );
  const to = others.find((node) => node.name === toName) ?? others[0];

  if (!open || !from) return null;

  const aPorts = cable === 'console' ? consolePorts(from) : freeInterfaces(from).map((port) => port.name);
  const bPorts = to ? (cable === 'console' ? consolePorts(to) : freeInterfaces(to).map((port) => port.name)) : [];
  const aChosen = aPorts.includes(aPort) ? aPort : (defaultInterface(from, cable) ?? aPorts[0] ?? '');
  const bChosen = to && bPorts.includes(bPort) ? bPort : to ? (defaultInterface(to, cable) ?? bPorts[0] ?? '') : '';

  const refusal =
    cable === 'console'
      ? (consoleRefusal(from) ?? (to ? consoleRefusal(to) : null))
      : others.length === 0
        ? 'Every other device in this lab has all its ports cabled.'
        : null;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={`Cable ${from.name}`}
      description="Both ends are listed with the ports that are still free."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            loading={busy}
            disabled={!to || aChosen === '' || bChosen === '' || refusal !== null}
            onClick={() =>
              to &&
              onConnect({
                a: { node: from.name, interface: aChosen },
                b: { node: to.name, interface: bChosen },
                cable,
              })
            }
          >
            Connect
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Field label="Cable">
          <Select
            value={cable}
            onChange={(event) => setCable(event.target.value as CableKind)}
            options={CABLES}
          />
        </Field>
        <Field label={`Port on ${from.name}`}>
          <Select
            value={aChosen}
            onChange={(event) => setAPort(event.target.value)}
            options={aPorts.map((name) => ({ value: name, label: name }))}
          />
        </Field>
        <Field label="To device">
          <Select
            value={to?.name ?? ''}
            onChange={(event) => {
              setToName(event.target.value);
              setBPort('');
            }}
            options={others.map((node) => ({ value: node.name, label: node.name }))}
          />
        </Field>
        <Field label={to ? `Port on ${to.name}` : 'Port'}>
          <Select
            value={bChosen}
            onChange={(event) => setBPort(event.target.value)}
            options={bPorts.map((name) => ({ value: name, label: name }))}
          />
        </Field>
      </div>
      {refusal && <p className={styles.refusal}>{refusal}</p>}
      {error && <p className={styles.refusal}>{error}</p>}
    </Dialog>
  );
}

/** The console pseudo-ports, which are not in the interface list. */
function consolePorts(node: NodeView): string[] {
  if (node.cli === 'ios') return ['Console'];
  if (node.cli === 'host') return ['RS232'];
  return [];
}
