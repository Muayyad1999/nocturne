import { CircleQuestionMark, Funnel, Info } from 'lucide-react';
import { useState } from 'react';
import { Button, IconButton } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { ConfirmDialog, Dialog, confirm } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Kbd } from '@/design/Kbd';
import { KeyValue } from '@/design/KeyValue';
import { Popover } from '@/design/Popover';
import { Select } from '@/design/Select';
import { StatusPill } from '@/design/StatusPill';
import { toast } from '@/design/Toast';
import { Tooltip } from '@/design/Tooltip';
import { Example, Section } from '../parts';
import styles from '../Showcase.module.css';

export function Overlays() {
  const [dialog, setDialog] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [confirmReboot, setConfirmReboot] = useState(false);
  const [rebooting, setRebooting] = useState(false);
  const [drawer, setDrawer] = useState(false);

  return (
    <>
      <Section
        title="Tooltip and Popover"
        description="A tooltip is a hint on hover and keyboard focus, never the only way to learn something. A popover holds a little interactive content; Escape or a press outside closes it."
      >
        <Example title="Tooltip">
          <Tooltip content="Shown after a short delay">
            <Button>Hover or focus me</Button>
          </Tooltip>
          <Tooltip
            content={
              <>
                Toggle assistant <Kbd shortcut="mod+j" />
              </>
            }
            placement="bottom"
          >
            <Button variant="ghost">With a shortcut</Button>
          </Tooltip>
          <IconButton icon={CircleQuestionMark} aria-label="Help" />
        </Example>
        <Example title="Popover">
          <Popover aria-label="Filters" trigger={<Button icon={Funnel}>Filters</Button>}>
            {(close) => (
              <div className={styles.stack} style={{ width: 260, padding: 12 }}>
                <Field label="Site">
                  <Select options={[{ value: 'all', label: 'All sites' }, { value: 'dc1', label: 'Amman DC1' }]} />
                </Field>
                <Checkbox defaultChecked>Only devices with alerts</Checkbox>
                <div className={styles.inline} style={{ justifyContent: 'flex-end' }}>
                  <Button size="sm" variant="ghost" onClick={close}>
                    Reset
                  </Button>
                  <Button size="sm" variant="primary" onClick={close}>
                    Apply
                  </Button>
                </div>
              </div>
            )}
          </Popover>
          <Popover aria-label="About this metric" placement="top" trigger={<IconButton icon={Info} aria-label="About this metric" />}>
            <p style={{ width: 240, padding: 12 }} className={styles.note}>
              Throughput is the sum of inbound and outbound traffic on the uplinks, averaged over five minutes.
            </p>
          </Popover>
        </Example>
        <Example title="Toast: bottom right, up to four; errors stay">
          <Button size="sm" onClick={() => toast.success('Backup saved', { description: 'core-sw-01 running-config' })}>
            Success
          </Button>
          <Button size="sm" onClick={() => toast.info('Discovery started', { description: 'Scanning 192.0.2.0/24' })}>
            Info
          </Button>
          <Button size="sm" onClick={() => toast.warning('3 devices did not answer')}>
            Warning
          </Button>
          <Button
            size="sm"
            onClick={() =>
              toast.error('The change was not applied', {
                description: 'edge-fw-01 refused the connection. Check that SSH is enabled and try again.',
                action: { label: 'Retry', onSelect: () => toast.info('Retrying') },
              })
            }
          >
            Error with action
          </Button>
        </Example>
      </Section>

      <Section
        title="Dialog and Drawer"
        description="Modal: focus moves in, Tab stays inside, Escape closes, and focus returns to where it was. A destructive confirmation has a clearly red button, or asks the user to type what they are about to affect."
      >
        <Example title="Dialog">
          <Button onClick={() => setDialog(true)}>Open dialog</Button>
          <Dialog
            open={dialog}
            onClose={() => setDialog(false)}
            title="Add device"
            description="Nocturne connects once to read the platform and the interfaces."
            footer={
              <>
                <Button onClick={() => setDialog(false)}>Cancel</Button>
                <Button variant="primary" onClick={() => setDialog(false)}>
                  Add device
                </Button>
              </>
            }
          >
            <div className={styles.stack}>
              <Field label="Address" hint="IPv4, IPv6 or a hostname." required>
                <Input mono data-autofocus placeholder="192.0.2.10" />
              </Field>
              <Field label="Credential">
                <Select options={[{ value: 'default', label: 'Default SSH credential' }, { value: 'lab', label: 'Lab' }]} />
              </Field>
            </div>
          </Dialog>
        </Example>
        <Example title="Confirm: red button, or typed">
          <Button onClick={() => setConfirmDelete(true)}>Delete backups</Button>
          <ConfirmDialog
            open={confirmDelete}
            onClose={() => setConfirmDelete(false)}
            onConfirm={() => {
              setConfirmDelete(false);
              toast.success('3 backups deleted');
            }}
            title="Delete 3 backups?"
            description="The backups of core-sw-01 from before 1 October will be removed. This cannot be undone."
            confirmLabel="Delete"
            tone="danger"
          />
          <Button onClick={() => setConfirmReboot(true)}>Reboot device</Button>
          <ConfirmDialog
            open={confirmReboot}
            onClose={() => setConfirmReboot(false)}
            onConfirm={() => {
              setRebooting(true);
              setTimeout(() => {
                setRebooting(false);
                setConfirmReboot(false);
                toast.success('Reboot command sent to edge-fw-01');
              }, 1200);
            }}
            title="Reboot edge-fw-01?"
            description="The firewall will be unreachable for about four minutes and every session through it will drop."
            confirmLabel="Reboot"
            tone="danger"
            confirmText="edge-fw-01"
            pending={rebooting}
          />
          <Button
            variant="ghost"
            onClick={async () => {
              const yes = await confirm({
                title: 'Discard the draft?',
                description: 'Your unsaved change request will be lost.',
                confirmLabel: 'Discard',
                tone: 'danger',
              });
              toast.info(yes ? 'Draft discarded' : 'Kept the draft');
            }}
          >
            confirm()
          </Button>
        </Example>
        <Example title="Drawer">
          <Button onClick={() => setDrawer(true)}>Open drawer</Button>
          <Drawer
            open={drawer}
            onClose={() => setDrawer(false)}
            title="core-sw-01"
            description="Amman DC1 · Cisco IOS-XE"
            footer={
              <>
                <Button onClick={() => setDrawer(false)}>Close</Button>
                <Button variant="primary" onClick={() => setDrawer(false)}>
                  Open device
                </Button>
              </>
            }
          >
            <div className={styles.stack}>
              <StatusPill tone="ok">Up</StatusPill>
              <KeyValue
                items={[
                  { label: 'Address', value: '192.0.2.1', mono: true },
                  { label: 'Serial', value: 'FOC2231X0AB', mono: true },
                  { label: 'Interfaces', value: '48 up, 2 down' },
                ]}
              />
            </div>
          </Drawer>
        </Example>
      </Section>
    </>
  );
}
