/**
 * The ISP settings: the business's calendar first, then the terms a new
 * invoice is written with.
 *
 * **The calendar is the one that matters.** `utc_offset_minutes` is where
 * the business's midnight is, and four things hang off it: where a billing
 * month begins and ends, which day a due date is, when that day is over (so,
 * what is overdue), and which year a number is counted in. It is a fixed
 * offset from UTC and not a named zone — daylight saving is not modelled —
 * and it is the business's, not this browser's: every date these screens
 * take or show is read in it (`calendar.ts`). A fresh installation is on
 * UTC, which is right for nobody east or west of it, so the first thing here
 * is one button that sets it to this computer's own offset.
 *
 * Changing it moves nothing that already exists. An invoice remembers the
 * calendar it was written in, and its period and due date stay days of that
 * one; the change applies to what is drafted, issued and aged from now on.
 * That is said beside the control rather than discovered afterwards.
 *
 * The engine's `settings.set` takes any value, so a calendar outside
 * UTC−12:00…UTC+14:00 can be stored by a script; the engine then refuses to
 * bill, age or read an invoice until it is put right. This screen says so,
 * shows what is stored, and is the place to fix it.
 */

import { MonitorDown } from 'lucide-react';
import { useEffect, useState } from 'react';
import type { IspSettings } from '@/api/types/isp';
import { Button } from '@/design/Button';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { toast } from '@/design/Toast';
import {
  computerOffsetMinutes,
  dayTimeText,
  isOffset,
  MAX_OFFSET_MINUTES,
  MIN_OFFSET_MINUTES,
  offsetChoices,
  offsetName,
} from './calendar';
import { useIspSettings, useSaveIspSettings, SETTINGS_SECTION } from './data';
import { parseBands, parseWholeNumber } from './records';
import styles from './isp.module.css';

export function SettingsTab() {
  const settings = useIspSettings();
  const save = useSaveIspSettings();

  if (settings.isError) {
    return (
      <div className={styles.scrollPane}>
        <ErrorState
          title="The ISP settings could not be read"
          error={settings.error}
          onRetry={() => void settings.refetch()}
        />
      </div>
    );
  }
  if (!settings.data) {
    return (
      <div className={styles.scrollPane}>
        <Skeleton shape="block" height={220} />
        <Skeleton shape="block" height={180} />
      </div>
    );
  }

  const set = (patch: Partial<IspSettings>, done: string, detail?: string) =>
    save.mutate(
      { section: SETTINGS_SECTION, value: patch as Record<string, unknown> },
      { onSuccess: () => toast.success(done, detail ? { description: detail } : undefined) },
    );

  return (
    <div className={styles.scrollPane}>
      {save.error && <p className={styles.error}>{save.error.message}</p>}
      <CalendarPanel
        stored={settings.data.utc_offset_minutes}
        saving={save.isPending}
        onChoose={(minutes) =>
          set(
            { utc_offset_minutes: minutes },
            `The business’s calendar is now ${offsetName(minutes)}`,
            'Months begin and days end at its midnight from now on. Invoices already written keep the calendar they were written in.',
          )
        }
      />
      <TermsPanel settings={settings.data} saving={save.isPending} set={set} />
    </div>
  );
}

function CalendarPanel({
  stored,
  saving,
  onChoose,
}: {
  stored: number;
  saving: boolean;
  onChoose: (minutes: number) => void;
}) {
  const valid = isOffset(stored);
  const computer = computerOffsetMinutes();
  // A computer on an offset the engine would refuse cannot be offered.
  const offerable = isOffset(computer);
  const choices = offsetChoices();
  // An offset that is valid but not a quarter hour (a script set it) is
  // still the calendar, and has to be in the list to be shown as chosen.
  const listed = choices.some((choice) => choice.value === stored);

  return (
    <Panel title="The business’s calendar" padding="none">
      <div className={styles.panelBody}>
        <p className={styles.note}>
          Where the business’s midnight is. It decides where a billing month begins and ends, which
          day an invoice falls due on and when that day is over, and which year a number is counted
          in. Every date on these screens is a day of this calendar — not of whichever computer the
          page is open on.
        </p>

        {valid ? (
          <div className={styles.stats}>
            <div className={styles.statGroup}>
              <span className={styles.statLabel}>Set to</span>
              <span className={styles.statValue}>{offsetName(stored)}</span>
              <span className={styles.quiet}>
                right now it is {dayTimeText(Date.now(), stored)} there
              </span>
            </div>
            <div className={styles.statGroup}>
              <span className={styles.statLabel}>This computer</span>
              <span className={styles.statValue}>
                {offerable ? offsetName(computer) : `${computer} minutes`}
              </span>
              <span className={styles.quiet}>
                {computer === stored
                  ? 'the same as the business’s calendar'
                  : 'not the business’s calendar: a date typed here is still read in the business’s'}
              </span>
            </div>
          </div>
        ) : (
          <p className={styles.error}>
            The setting holds {String(stored)}, which is not a calendar: it has to be a whole number
            of minutes between {MIN_OFFSET_MINUTES} and {MAX_OFFSET_MINUTES} (UTC−12:00 to
            UTC+14:00). Until it is put right nothing is billed, aged or called overdue, and no
            date on these screens can be shown or read.
          </p>
        )}

        <div className={styles.inlineFields}>
          <Field
            label="Offset from UTC"
            hint="A fixed offset, not a zone: daylight saving is not modelled. A business that moves its clocks keeps its books on standard time, or changes this when the clocks change."
          >
            <Select
              value={valid ? String(stored) : ''}
              disabled={saving}
              onChange={(event) => {
                const minutes = Number(event.target.value);
                if (event.target.value !== '' && isOffset(minutes) && minutes !== stored) {
                  onChoose(minutes);
                }
              }}
              options={[
                ...(valid
                  ? []
                  : [{ value: '', label: `${String(stored)} — not a calendar`, disabled: true }]),
                ...(valid && !listed ? [{ value: String(stored), label: offsetName(stored) }] : []),
                ...choices.map((choice) => ({ value: String(choice.value), label: choice.label })),
              ]}
            />
          </Field>
          <Button
            icon={MonitorDown}
            loading={saving}
            disabled={!offerable || computer === stored}
            onClick={() => onChoose(computer)}
          >
            {offerable
              ? `Use this computer’s offset (${offsetName(computer)})`
              : 'Use this computer’s offset'}
          </Button>
        </div>

        <p className={styles.quiet}>
          Changing it moves no date on an invoice that already exists: each invoice remembers the
          calendar it was written in. It applies to what is drafted, issued and aged from now on.
        </p>
      </div>
    </Panel>
  );
}

function TermsPanel({
  settings,
  saving,
  set,
}: {
  settings: IspSettings;
  saving: boolean;
  set: (patch: Partial<IspSettings>, done: string, detail?: string) => void;
}) {
  return (
    <Panel title="Invoices and arrears" padding="none">
      <div className={styles.panelBody}>
        <div className={styles.pair}>
          <TextSetting
            label="Payment terms, in days"
            hint="How many whole days after it is issued an invoice falls due, when no due date is given. It is overdue once that day is over."
            stored={String(settings.payment_terms_days)}
            saving={saving}
            parse={(text) => parseWholeNumber(text, 0, 3650)}
            invalid="A whole number of days, from 0 to 3650."
            onSave={(days) =>
              set(
                { payment_terms_days: days },
                `Invoices now fall due ${days} day${days === 1 ? '' : 's'} after issue`,
                'Invoices already issued keep the due date they were issued with.',
              )
            }
          />
          <TextSetting
            label="Invoice number prefix"
            hint="Put in front of every invoice and credit note number from now on: ACME- gives ACME-2026-0001. Numbers already given out do not change."
            stored={settings.invoice_number_prefix}
            saving={saving}
            parse={(text) => (text.length <= 16 ? text : null)}
            invalid="At most 16 characters."
            onSave={(prefix) =>
              set(
                { invoice_number_prefix: prefix },
                prefix === '' ? 'New numbers have no prefix' : `New numbers start with ${prefix}`,
              )
            }
          />
        </div>
        <div className={styles.pair}>
          <TextSetting
            label="Arrears bands, in days overdue"
            hint="Where one age band of the arrears report ends and the next begins: 30, 60, 90 gives 1–30, 31–60, 61–90 and over 90 days."
            stored={settings.arrears_buckets.join(', ')}
            saving={saving}
            parse={parseBands}
            invalid="Whole numbers of days greater than zero, each larger than the one before, separated by commas."
            onSave={(bands) =>
              set({ arrears_buckets: bands }, `Arrears are aged at ${bands.join(', ')} days`)
            }
          />
          <Field
            label="What counts as a customer’s usage"
            hint="Which direction of their serving interface is their traffic depends on which side of the link it is. Every usage figure says which it used."
          >
            <Select
              value={['in', 'out'].includes(settings.usage_basis) ? settings.usage_basis : 'greater'}
              disabled={saving}
              onChange={(event) =>
                set({ usage_basis: event.target.value }, 'Usage figures use that direction now')
              }
              options={[
                { value: 'greater', label: 'The greater of the two directions' },
                { value: 'in', label: 'Inbound on the interface' },
                { value: 'out', label: 'Outbound on the interface' },
              ]}
            />
          </Field>
        </div>
        <div className={styles.pair}>
          <TextSetting
            label="Usage window, in hours"
            hint="How old an interface’s latest reading may be and still be taken for what a customer is using. Monitoring keeps the last reading of a device it can no longer reach; an older one is shown as no recent reading, not as a figure."
            stored={String(settings.usage_window_hours)}
            saving={saving}
            parse={(text) => parseWholeNumber(text, 1, 8760)}
            invalid="A whole number of hours, from 1 to 8760."
            onSave={(hours) =>
              set(
                { usage_window_hours: hours },
                `A latest reading counts for ${hours} hour${hours === 1 ? '' : 's'} now`,
              )
            }
          />
        </div>
        <p className={styles.quiet}>
          None of these rewrites anything already issued: an issued invoice is frozen, with the
          due date and the number it was given.
        </p>
      </div>
    </Panel>
  );
}

/**
 * A setting typed as text and saved when the field is left (or Enter is
 * pressed), if it changed and can be read. What cannot be read is said, and
 * is not saved as something else.
 */
function TextSetting<T>({
  label,
  hint,
  stored,
  saving,
  parse,
  invalid,
  onSave,
}: {
  label: string;
  hint: string;
  stored: string;
  saving: boolean;
  parse: (text: string) => T | null;
  invalid: string;
  onSave: (value: T) => void;
}) {
  const [text, setText] = useState(stored);
  useEffect(() => setText(stored), [stored]);
  const parsed = parse(text.trim());
  const commit = () => {
    if (parsed === null || text.trim() === stored) return;
    onSave(parsed);
  };
  return (
    <Field label={label} hint={hint} error={parsed === null ? invalid : undefined}>
      <Input
        mono
        value={text}
        invalid={parsed === null}
        disabled={saving}
        onChange={(event) => setText(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === 'Enter') commit();
        }}
      />
    </Field>
  );
}
