/**
 * Free space in a block: where the next child subnet would fit, and which
 * addresses the engine would hand out.
 *
 * Neither answer creates anything, and neither is a fact. `ipam.next_address`
 * returns a `basis` — what the answer rests on — and a breakdown of what it
 * skipped, and both are on screen: an address nothing has answered on is
 * "not seen", which is not the same as "yours to use". The suggestion is
 * therefore offered as a suggestion, with a copy button, not as an assignment.
 */

import { Check, Copy } from 'lucide-react';
import { useState } from 'react';
import type { NextAddresses, NextPrefix, PrefixRow } from '@/api/types/ipam';
import type { RpcError } from '@/api/errors';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Skeleton } from '@/design/Skeleton';
import { toast } from '@/design/Toast';
import { copyText } from '@/lib/clipboard';
import { formatCount } from '@/lib/format';
import { humaniseBasis, nextAddressSummary, skippedLines } from './addresses';
import styles from './Dialogs.module.css';

export interface FreeSpaceDialogProps {
  open: boolean;
  prefix: PrefixRow | null;
  /** The asked-for child length and address count, owned by the page. */
  length: number;
  count: number;
  onLength: (length: number) => void;
  onCount: (count: number) => void;
  nextPrefix: NextPrefix | undefined;
  nextPrefixError: RpcError | null;
  nextAddresses: NextAddresses | undefined;
  nextAddressError: RpcError | null;
  loading: boolean;
  onClose: () => void;
  /** Takes the suggested child block to the "add a block" form. */
  onUsePrefix: (cidr: string) => void;
  /** Takes the first suggested address to the reservation form. */
  onReserve: (ip: string) => void;
}

export function FreeSpaceDialog({
  open,
  prefix,
  length,
  count,
  onLength,
  onCount,
  nextPrefix,
  nextPrefixError,
  nextAddresses,
  nextAddressError,
  loading,
  onClose,
  onUsePrefix,
  onReserve,
}: FreeSpaceDialogProps) {
  const [copied, setCopied] = useState(false);

  const copy = async (text: string) => {
    const ok = await copyText(text);
    if (ok) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1_500);
    } else {
      toast.error('This browser would not let Nocturne write to the clipboard');
    }
  };

  const skipped = nextAddresses ? skippedLines(nextAddresses) : [];

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={prefix ? `Free space in ${prefix.cidr}` : 'Free space'}
      description="Nothing here is created or assigned. Both answers say what they rest on."
      footer={
        <Button variant="secondary" onClick={onClose}>
          Close
        </Button>
      }
    >
      <div className={styles.form}>
        <section className={styles.form} aria-label="The next free child block">
          <Field label="Next free child block of length" hint="Nothing is created.">
            <Input
              mono
              type="number"
              min={1}
              max={prefix?.cidr.includes(':') ? 128 : 32}
              value={String(length)}
              onChange={(event) => onLength(Number(event.target.value))}
            />
          </Field>

          {loading && !nextPrefix ? (
            <Skeleton width={220} />
          ) : nextPrefixError ? (
            <p className={styles.error}>{nextPrefixError.message}</p>
          ) : nextPrefix ? (
            <>
              <div className={styles.answer}>
                {nextPrefix.cidr === null ? (
                  <p className={styles.note}>
                    Nothing of that size fits inside {nextPrefix.parent} any more.
                  </p>
                ) : (
                  <>
                    <span className={styles.answerValue}>{nextPrefix.cidr}</span>
                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={() => onUsePrefix(nextPrefix.cidr ?? '')}
                    >
                      Document it
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      icon={copied ? Check : Copy}
                      onClick={() => void copy(nextPrefix.cidr ?? '')}
                    >
                      Copy
                    </Button>
                  </>
                )}
              </div>
              <p className={styles.quiet}>{nextPrefix.note}</p>
              {nextPrefix.free_space.length > 0 && (
                <p className={styles.quiet}>
                  Free child space: {nextPrefix.free_space.slice(0, 10).join(', ')}
                  {nextPrefix.free_space.length > 10 &&
                    ` and ${formatCount(nextPrefix.free_space.length - 10)} more`}
                </p>
              )}
            </>
          ) : null}
        </section>

        <hr className={styles.rule} />

        <section className={styles.form} aria-label="Free addresses">
          <Field
            label="Free addresses to suggest"
            hint="The network and broadcast addresses, the documented gateway, the DHCP range, anything documented and anything seen recently are all skipped."
          >
            <Input
              mono
              type="number"
              min={1}
              max={64}
              value={String(count)}
              onChange={(event) => onCount(Number(event.target.value))}
            />
          </Field>

          {loading && !nextAddresses ? (
            <Skeleton width={260} />
          ) : nextAddressError ? (
            <p className={styles.error}>{nextAddressError.message}</p>
          ) : nextAddresses ? (
            <>
              <p className={styles.note}>{nextAddressSummary(nextAddresses)}</p>
              <ul className={styles.addresses}>
                {nextAddresses.addresses.map((ip) => (
                  <li key={ip}>{ip}</li>
                ))}
              </ul>
              {nextAddresses.addresses.length > 0 && (
                <div className={styles.answer}>
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => onReserve(nextAddresses.addresses[0] ?? '')}
                  >
                    Reserve {nextAddresses.addresses[0]}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={copied ? Check : Copy}
                    onClick={() => void copy(nextAddresses.addresses.join('\n'))}
                  >
                    Copy {nextAddresses.addresses.length === 1 ? 'it' : 'them'}
                  </Button>
                </div>
              )}
              {/* The engine's own sentence about what this answer rests on,
                  with its epoch stamp turned into a readable time. */}
              <p className={styles.basis}>{humaniseBasis(nextAddresses.basis)}</p>
              {skipped.length > 0 ? (
                <ul className={styles.skipped}>
                  {skipped.map((line) => (
                    <li key={line.label}>
                      <span>{line.label}</span>
                      <span className={styles.skippedCount}>{formatCount(line.count)}</span>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className={styles.quiet}>Nothing in this block had to be skipped.</p>
              )}
            </>
          ) : null}
        </section>
      </div>
    </Dialog>
  );
}
