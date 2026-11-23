/**
 * Reports for the people who ask for reports.
 *
 * Seven kinds and four formats, all rendered by the engine — this tab chooses
 * and shows, and writes none of the document. What it adds is the one thing a
 * reader cannot get from the text: a statement of **coverage**. A report with
 * no rows because nothing happened and a report with no rows because a source
 * could not be read are different documents, and `reportCoverage` makes them
 * read differently.
 *
 * A kind whose source is not in this build is still offered, greyed, with the
 * engine's own reason: "not available in this build" is a fact worth seeing,
 * and the engine generates such a report as a document that says so rather
 * than as an empty table.
 */

import { FileDown, FileText, Printer, Save, Trash2 } from 'lucide-react';
import { useState } from 'react';
import type { Format, ReportKind, ReportKindInfo } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { download } from '@/lib/download';
import { formatBytes, formatCount, plural } from '@/lib/format';
import { formatDateTime, formatTimestamp } from '@/lib/time';
import { useAuthor } from './author';
import {
  useDeleteReport,
  useReport,
  useReportKinds,
  useSaveReport,
  useSavedReport,
  useSavedReports,
} from './data';
import styles from './Reports.module.css';
import opsStyles from './Ops.module.css';
import { PERIODS, instant, periodChoice, reportCoverage } from './ops';
import { usePrintFrame } from './PrintFrame';
import { UnavailableNote } from './Unavailable';

const FORMATS: readonly { value: Format; label: string }[] = [
  { value: 'markdown', label: 'Markdown' },
  { value: 'html', label: 'HTML' },
  { value: 'csv', label: 'CSV' },
  { value: 'json', label: 'JSON' },
];

/** `<input type="datetime-local">` wants local wall-clock text. */
function toLocalInput(at: number): string {
  const date = new Date(at - new Date(at).getTimezoneOffset() * 60_000);
  return date.toISOString().slice(0, 16);
}

export function ReportsTab() {
  const kinds = useReportKinds();
  const [kind, setKind] = useState<ReportKind>('inventory');
  const [format, setFormat] = useState<Format>('markdown');
  const [periodId, setPeriodId] = useState('7d');
  const [since, setSince] = useState(() => periodChoice('7d').since(Date.now()));
  const [until, setUntil] = useState(() => Date.now());
  const [openSaved, setOpenSaved] = useState<string | null>(null);
  const [author] = useAuthor();
  const { printDocument, printFrame } = usePrintFrame();

  const info = kinds.data?.kinds.find((entry) => entry.id === kind);
  const wantsPeriod = info?.period ?? false;
  const report = useReport({
    kind,
    format,
    ...(wantsPeriod ? { since, until } : {}),
  });
  const save = useSaveReport();
  const saved = useSavedReport(openSaved);

  if (kinds.isError) {
    return (
      <ErrorState
        title="The report kinds could not be read"
        error={kinds.error}
        onRetry={() => void kinds.refetch()}
      />
    );
  }

  const shown = openSaved !== null ? saved.data : report.data;
  // A kept report is in the format it was kept in, whatever the selector says.
  const printable = (openSaved !== null ? saved.data?.format : format) === 'html';
  const coverage = openSaved === null && report.data ? reportCoverage(report.data) : null;

  return (
    <div className={opsStyles.columns}>
      <div className={styles.main}>
        <Toolbar>
          <Select
            aria-label="Report"
            size="sm"
            className={opsStyles.filter}
            value={kind}
            onChange={(event) => {
              setKind(event.target.value as ReportKind);
              setOpenSaved(null);
            }}
            options={(kinds.data?.kinds ?? []).map((entry) => ({
              value: entry.id,
              label: entry.available ? entry.title : `${entry.title} (not in this build)`,
            }))}
          />
          <Select
            aria-label="Format"
            size="sm"
            className={opsStyles.narrow}
            value={format}
            options={FORMATS}
            onChange={(event) => setFormat(event.target.value as Format)}
          />
          {wantsPeriod && (
            <>
              <Select
                aria-label="Period"
                size="sm"
                className={opsStyles.narrow}
                value={periodId}
                options={[
                  ...PERIODS.map((period) => ({ value: period.id, label: period.label })),
                  { value: 'custom', label: 'Exactly…' },
                ]}
                onChange={(event) => {
                  const id = event.target.value;
                  setPeriodId(id);
                  if (id !== 'custom') {
                    const now = Date.now();
                    setSince(periodChoice(id).since(now));
                    setUntil(now);
                  }
                }}
              />
              {periodId === 'custom' && (
                <>
                  <Input
                    size="sm"
                    type="datetime-local"
                    aria-label="From"
                    className={opsStyles.when}
                    value={toLocalInput(since)}
                    onChange={(event) => {
                      const at = new Date(event.target.value).getTime();
                      if (Number.isFinite(at)) setSince(at);
                    }}
                  />
                  <Input
                    size="sm"
                    type="datetime-local"
                    aria-label="To"
                    className={opsStyles.when}
                    value={toLocalInput(until)}
                    onChange={(event) => {
                      const at = new Date(event.target.value).getTime();
                      if (Number.isFinite(at)) setUntil(at);
                    }}
                  />
                </>
              )}
            </>
          )}
          <ToolbarSpacer />
          {printable && (
            <Button
              variant="ghost"
              icon={Printer}
              disabled={!shown}
              onClick={() => {
                if (shown) printDocument(shown.content);
              }}
            >
              Print
            </Button>
          )}
          <Button
            variant="secondary"
            icon={FileDown}
            disabled={!shown}
            onClick={() => {
              if (!shown) return;
              const filename = 'filename' in shown ? shown.filename : `${shown.kind}.${format}`;
              download(filename, shown.content);
            }}
          >
            Download
          </Button>
          <Button
            variant="primary"
            icon={Save}
            loading={save.isPending}
            disabled={openSaved !== null || !report.data}
            onClick={() =>
              save.mutate(
                { kind, format, author, ...(wantsPeriod ? { since, until } : {}) },
                { onSuccess: (stored) => setOpenSaved(stored.id) },
              )
            }
          >
            Keep this report
          </Button>
        </Toolbar>

        {info && openSaved === null && (
          <p className={opsStyles.sentence}>{info.description}</p>
        )}

        {info && !info.available && (
          <UnavailableNote
            reason="not_installed"
            about={info.title}
            message={
              info.unavailable_reason ??
              `\`${info.needs ?? 'the feature it needs'}\` is not part of this build, so the report says so instead of showing an empty table.`
            }
          />
        )}

        {openSaved !== null ? (
          <SavedReportView id={openSaved} onBack={() => setOpenSaved(null)} />
        ) : (
          <>
            {coverage && (
              <ul className={opsStyles.sentences} aria-label="What this report covers">
                {coverage.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            )}

            {report.data && report.data.unavailable.length > 0 && (
              <div className={styles.gaps}>
                {report.data.unavailable.map((entry) => (
                  <UnavailableNote
                    key={entry.section}
                    size="inline"
                    about={entry.section}
                    reason={entry.reason}
                    message={entry.message}
                  />
                ))}
              </div>
            )}

            {report.isError && (
              <ErrorState
                size="compact"
                title="That report could not be generated"
                error={report.error}
                onRetry={() => void report.refetch()}
              />
            )}
            {report.isPending && (
              <div aria-busy="true">
                <Skeleton shape="block" height={320} />
              </div>
            )}
            {report.data && (
              <>
                <p className={opsStyles.facts}>
                  <span className={opsStyles.mono}>{report.data.filename}</span>
                  <span>{formatBytes(new Blob([report.data.content]).size)}</span>
                  <span>{plural(report.data.rows, 'data row')}</span>
                  {report.data.period && (
                    <span title={`${formatTimestamp(report.data.period.since)} → ${formatTimestamp(report.data.period.until)}`}>
                      {formatDateTime(report.data.period.since)} → {formatDateTime(report.data.period.until)}
                    </span>
                  )}
                  <span className={opsStyles.muted}>
                    generated {formatDateTime(instant(report.data.generated_at))}
                  </span>
                </p>
                <Preview content={report.data.content} format={format} title={report.data.title} />
              </>
            )}
          </>
        )}
      </div>

      <aside className={styles.aside}>
        <Panel title="What this build can report on">
          {kinds.isPending && <Skeleton shape="block" height={160} />}
          <ul className={styles.kinds}>
            {(kinds.data?.kinds ?? []).map((entry) => (
              <KindRow
                key={entry.id}
                info={entry}
                current={entry.id === kind && openSaved === null}
                onChoose={() => {
                  setKind(entry.id);
                  setOpenSaved(null);
                }}
              />
            ))}
          </ul>
        </Panel>
        <SavedReports openSaved={openSaved} onOpen={setOpenSaved} />
      </aside>
      {printFrame}
    </div>
  );
}

function KindRow({
  info,
  current,
  onChoose,
}: {
  info: ReportKindInfo;
  current: boolean;
  onChoose: () => void;
}) {
  return (
    <li className={styles.kind} data-current={current || undefined}>
      <button type="button" className={styles.kindButton} onClick={onChoose}>
        <span className={styles.kindTitle}>
          {info.title}
          {info.period && (
            <Badge tone="neutral" variant="outline">
              period
            </Badge>
          )}
          {!info.available && (
            <Badge tone="idle" variant="outline" title={info.unavailable_reason ?? undefined}>
              no source
            </Badge>
          )}
        </span>
        <span className={styles.kindAbout}>{info.description}</span>
      </button>
    </li>
  );
}

function Preview({ content, format, title }: { content: string; format: Format; title: string }) {
  if (format === 'html') {
    // Self-contained, with no scripts, so it is shown exactly as it prints —
    // in a frame that is allowed nothing. Such a frame cannot be asked to
    // print either; that is what `PrintFrame.tsx` is for.
    return <iframe className={styles.frame} title={`${title} (preview)`} sandbox="" srcDoc={content} />;
  }
  return <CodeBlock code={content} title={title} maxHeight={520} wrap={format === 'markdown'} />;
}

function SavedReportView({ id, onBack }: { id: string; onBack: () => void }) {
  const saved = useSavedReport(id);
  if (saved.isPending) return <Skeleton shape="block" height={320} />;
  if (saved.isError) {
    return <ErrorState size="compact" title="That saved report could not be read" error={saved.error} />;
  }
  if (!saved.data) return null;
  const report = saved.data;
  return (
    <>
      <p className={opsStyles.facts}>
        <b>{report.title}</b>
        <span>{report.kind}</span>
        <span>{plural(report.rows, 'data row')}</span>
        <span className={opsStyles.muted}>
          generated {formatDateTime(instant(report.generated_at))}
          {report.author && ` by ${report.author}`}
        </span>
        <button type="button" className={styles.back} onClick={onBack}>
          Generate a new one instead
        </button>
      </p>
      {report.unavailable.length > 0 && (
        <div className={styles.gaps}>
          {report.unavailable.map((entry) => (
            <UnavailableNote
              key={entry.section}
              size="inline"
              about={entry.section}
              reason={entry.reason}
              message={`When this report was generated: ${entry.message}`}
            />
          ))}
        </div>
      )}
      <Preview content={report.content} format={(report.format as Format) ?? 'markdown'} title={report.title} />
    </>
  );
}

function SavedReports({
  openSaved,
  onOpen,
}: {
  openSaved: string | null;
  onOpen: (id: string | null) => void;
}) {
  const saved = useSavedReports();
  const remove = useDeleteReport();
  return (
    <Panel title="Kept reports" padding={saved.data?.reports.length ? 'none' : 'md'}>
      {saved.isPending && <Skeleton shape="block" height={80} />}
      {saved.data && saved.data.reports.length === 0 && (
        <EmptyState
          size="compact"
          icon={FileText}
          title="Nothing kept yet"
          description="A kept report stores the text the engine generated, so it stays exactly as it was when somebody was shown it."
        />
      )}
      {saved.data && saved.data.reports.length > 0 && (
        <ul className={styles.savedList}>
          {saved.data.reports.map((head) => (
            <li key={head.id} className={styles.savedRow} data-current={head.id === openSaved || undefined}>
              <button type="button" className={styles.savedOpen} onClick={() => onOpen(head.id)}>
                <span className={styles.savedTitle}>{head.title}</span>
                <span className={styles.savedMeta}>
                  {head.kind} · {head.format} · {plural(head.rows, 'row')} · {formatBytes(head.bytes)}
                  {head.unavailable.length > 0 && (
                    <span className={styles.savedGap}>
                      {' '}
                      · {formatCount(head.unavailable.length)} unread section
                    </span>
                  )}
                </span>
                <span className={styles.savedMeta}>{formatDateTime(instant(head.generated_at))}</span>
              </button>
              <Button
                size="sm"
                variant="ghost"
                icon={Trash2}
                aria-label={`Delete ${head.title}`}
                onClick={async () => {
                  const sure = await confirm({
                    title: 'Delete this kept report?',
                    description: 'Only the stored copy goes; it can be generated again from the records.',
                    confirmLabel: 'Delete it',
                    tone: 'danger',
                  });
                  if (!sure) return;
                  remove.mutate({ id: head.id });
                  if (head.id === openSaved) onOpen(null);
                }}
              />
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}
