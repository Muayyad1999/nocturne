/**
 * Turning what the network shows into records — the one call that does, and
 * the only place on this screen where evidence becomes a plan.
 *
 * `ipam.learn` previews unless it is told otherwise, so this dialog always
 * asks for the preview first and shows exactly what would be written, row by
 * row, with the engine's own notes. Reconciliation refuses to decide whether
 * an undocumented address is a device to reconfigure or a plan to update; this
 * is a person deciding the second, deliberately, with the list in front of
 * them. Nothing is touched on any device either way.
 */

import type { LearnPlan } from '@/api/types/ipam';
import type { RpcError } from '@/api/errors';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Switch } from '@/design/Switch';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount, plural } from '@/lib/format';
import { coverageReading } from './reconcile';
import styles from './Dialogs.module.css';

export interface LearnDialogProps {
  open: boolean;
  plan: LearnPlan | null;
  /** Turns a `lab:<id>` namespace into the lab's name. */
  siteLabel: (site: string | null) => string;
  includeArp: boolean;
  onIncludeArp: (include: boolean) => void;
  pending: boolean;
  error: RpcError | null;
  onClose: () => void;
  /** Runs `ipam.learn`; `commit` sends `dry_run: false`. */
  onRun: (commit: boolean) => void;
}

export function LearnDialog({
  open,
  plan,
  siteLabel,
  includeArp,
  onIncludeArp,
  pending,
  error,
  onClose,
  onRun,
}: LearnDialogProps) {
  const preview = plan?.dry_run === true ? plan : null;
  const done = plan?.dry_run === false ? plan : null;
  const coverage = plan ? coverageReading(plan.coverage) : null;
  // `created_*` counts what was *written*, so in a preview it is zero however
  // much the plan would create. The preview counts its own rows instead.
  const counts = plan
    ? plan.dry_run
      ? {
          prefixes: plan.prefixes.filter((row) => row.action === 'create').length,
          addresses: plan.addresses.filter((row) => row.action === 'create').length,
          vlans: plan.vlans.filter((row) => row.action === 'create').length,
        }
      : {
          prefixes: plan.created_prefixes,
          addresses: plan.created_addresses,
          vlans: plan.created_vlans,
        }
    : { prefixes: 0, addresses: 0, vlans: 0 };
  const toCreate = preview ? counts.prefixes + counts.addresses + counts.vlans : 0;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title="Document what the network shows"
      description="A preview first. Records created here are yours from then on: the network is checked against them, and nothing is sent to a device."
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Close
          </Button>
          <Button variant="secondary" loading={pending} onClick={() => onRun(false)}>
            Preview again
          </Button>
          <Button
            variant="primary"
            loading={pending}
            disabled={preview === null || toCreate === 0}
            onClick={() => onRun(true)}
          >
            {toCreate === 0 ? 'Nothing to create' : `Create ${formatCount(toCreate)} records`}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Switch checked={includeArp} onChange={(event) => onIncludeArp(event.target.checked)}>
          Also document the hosts that answered in ARP
        </Switch>

        {error && <p className={styles.error}>{error.message}</p>}

        {coverage && (
          <p className={styles.quiet}>
            {coverage.headline}. {coverage.detail}
          </p>
        )}

        {plan && (
          <>
            <div className={styles.counts}>
              <span>
                {done ? 'Created' : 'Would create'}: {plural(counts.prefixes, 'block')}
              </span>
              <span>{plural(counts.addresses, 'address', 'addresses')}</span>
              <span>{plural(counts.vlans, 'VLAN')}</span>
            </div>

            {plan.notes.length > 0 && (
              <ul className={styles.skipped}>
                {plan.notes.map((note) => (
                  <li key={note}>{note}</li>
                ))}
              </ul>
            )}

            {plan.prefixes.length > 0 && (
              <>
                <p className={styles.tableTitle}>
                  Blocks<span className={styles.quiet}> · {formatCount(plan.prefixes.length)}</span>
                </p>
              <div className={styles.scroll}>
                <table className={styles.table}>
                  <thead>
                    <tr>
                      <th scope="col">Block</th>
                      <th scope="col">Site</th>
                      <th scope="col">Seen on</th>
                      <th scope="col">Outcome</th>
                    </tr>
                  </thead>
                  <tbody>
                    {plan.prefixes.map((row) => (
                      <tr key={`${row.site ?? ''}-${row.cidr}`}>
                        <td className={styles.mono}>{row.cidr}</td>
                        <td>{siteLabel(row.site)}</td>
                        <td>
                          {row.from
                            .slice(0, 2)
                            .map((sighting) => `${sighting.device} (${sighting.interface})`)
                            .join(', ')}
                          {row.from.length > 2 && ` +${formatCount(row.from.length - 2)}`}
                        </td>
                        <td>
                          <Badge tone={row.action === 'create' ? 'ok' : 'idle'} variant="outline">
                            {row.action}
                          </Badge>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              </>
            )}

            {plan.addresses.length > 0 && (
              <>
                <p className={styles.tableTitle}>
                  Addresses
                  <span className={styles.quiet}> · {formatCount(plan.addresses.length)}</span>
                </p>
              <div className={styles.scroll}>
                <table className={styles.table}>
                  <thead>
                    <tr>
                      <th scope="col">Address</th>
                      <th scope="col">Block</th>
                      <th scope="col">Seen on</th>
                      <th scope="col">From</th>
                      <th scope="col">Read</th>
                      <th scope="col">Outcome</th>
                    </tr>
                  </thead>
                  <tbody>
                    {plan.addresses.map((row) => (
                      <tr key={`${row.scope}-${row.ip}-${row.device_id}-${row.interface}`}>
                        <td className={styles.mono}>{row.ip}</td>
                        <td className={styles.mono}>{row.prefix ?? '—'}</td>
                        <td>
                          {row.device}
                          {row.interface ? ` (${row.interface})` : ''}
                        </td>
                        <td>{row.kind}</td>
                        <td>
                          <TimeAgo at={row.collected_at} />
                        </td>
                        <td>
                          <Badge
                            tone={
                              row.action === 'create' ? 'ok' : row.action === 'skip' ? 'warn' : 'idle'
                            }
                            variant="outline"
                          >
                            {row.action}
                          </Badge>
                          {row.reason && <span className={styles.quiet}> {row.reason}</span>}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              </>
            )}

            {plan.vlans.length > 0 && (
              <>
                <p className={styles.tableTitle}>
                  VLANs<span className={styles.quiet}> · {formatCount(plan.vlans.length)}</span>
                </p>
              <div className={styles.scroll}>
                <table className={styles.table}>
                  <thead>
                    <tr>
                      <th scope="col">VLAN</th>
                      <th scope="col">Name</th>
                      <th scope="col">Site</th>
                      <th scope="col">On</th>
                      <th scope="col">Outcome</th>
                    </tr>
                  </thead>
                  <tbody>
                    {plan.vlans.map((row) => (
                      <tr key={`${row.site ?? ''}-${row.vid}`}>
                        <td className={styles.mono}>{row.vid}</td>
                        <td>{row.name}</td>
                        <td>{siteLabel(row.site)}</td>
                        <td>{row.devices.slice(0, 3).join(', ')}</td>
                        <td>
                          <Badge tone={row.action === 'create' ? 'ok' : 'idle'} variant="outline">
                            {row.action}
                          </Badge>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              </>
            )}

            {plan.prefixes.length === 0 &&
              plan.addresses.length === 0 &&
              plan.vlans.length === 0 && (
                <p className={styles.note}>
                  There is nothing to document: every address the devices report already has a
                  record, or nothing has been collected yet.
                </p>
              )}
          </>
        )}
      </div>
    </Dialog>
  );
}
