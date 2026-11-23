/**
 * What is inside an evidence item, one viewer per kind.
 *
 * - **Text** (a terminal capture, a piece of a log, text a person handed in)
 *   is preformatted and shown as text, with its control characters as
 *   pictures of themselves. It is never written into the terminal emulator,
 *   which would obey an escape sequence instead of showing it. A very long
 *   one is shown a page of lines at a time, and says which lines those are:
 *   an item can hold more lines than a window can lay out at once.
 * - **A drawing** is shown as an image and only as an image. A stored SVG is
 *   not sanitised — it is the record of what the engine returned — so the
 *   string goes to exactly two places in this file: {@link svgImageSrc},
 *   which makes the `data:` address of the `<img>`, and `lib/capture`, which
 *   decodes it as an image to make a PNG. It is never markup: no
 *   `innerHTML`, no frame, no `<object>`, no window opened on it. The
 *   "Download SVG" button saves it as *plain text*, so that a window which
 *   followed the link instead of saving it would show source, not run it.
 * - **A chart** is drawn again from the stored points with its holes left
 *   as holes, and the engine's statement of coverage is written out under
 *   it, because a line with a gap in it is easy to read as a line.
 */

import { ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, Copy, Download, ImageDown, Unplug } from 'lucide-react';
import { memo, useEffect, useMemo, useRef, useState } from 'react';
import type { EvidenceBody, EvidenceWithBody } from '@/api/types/ops';
import { Button, IconButton } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Icon } from '@/design/Icon';
import { LabPill } from '@/design/StatusPill';
import { TimeSeriesChart } from '@/design/TimeSeriesChart';
import { copyImageOrSave, savePicture, svgToPng, tellImageCopied } from '@/lib/capture';
import { download, tellSaved } from '@/lib/download';
import { formatCount, plural } from '@/lib/format';
import { formatDateTime } from '@/lib/time';
import styles from './Evidence.module.css';
import {
  coverageLines,
  evidenceFilename,
  gapSentence,
  picturedCharacters,
  storedChart,
  svgImageSrc,
  TEXT_PAGE_LINES,
  textLines,
  textPage,
  visibleText,
} from './evidenceReading';

/**
 * Memoised on the item: the page around it re-renders for every dialog it
 * opens, and a page of text is thousands of elements to go over again.
 */
export const EvidenceBodyView = memo(function EvidenceBodyView({
  item,
  linesAtOnce = TEXT_PAGE_LINES,
}: {
  item: EvidenceWithBody;
  /** How many lines of a text are on screen at once. The default is what a window lays out without a pause. */
  linesAtOnce?: number;
}) {
  const body = item.body;
  switch (body.type) {
    case 'text':
      return <TextBody item={item} text={body.text} linesAtOnce={linesAtOnce} />;
    case 'svg':
      return <DrawingBody item={item} svg={body.svg} />;
    case 'chart':
      return <ChartBody item={item} body={body} />;
    default:
      // A kind of content a newer engine stores and this interface does not
      // know: said, rather than shown as an empty box.
      return (
        <p className={styles.unknownBody} role="status">
          This item holds a kind of content this version of the interface cannot show (
          <code>{String((body as { type?: unknown }).type)}</code>). Its facts beside this are still the
          engine’s own.
        </p>
      );
  }
});

function TextBody({ item, text, linesAtOnce }: { item: EvidenceWithBody; text: string; linesAtOnce: number }) {
  const lines = useMemo(() => textLines(visibleText(text)), [text]);
  const pictured = useMemo(() => picturedCharacters(text), [text]);
  const [at, setAt] = useState(0);
  const page = useMemo(() => textPage(lines, at, linesAtOnce), [lines, at, linesAtOnce]);
  const viewer = useRef<HTMLDivElement>(null);
  const focusText = useRef(false);

  const show = (next: number) => {
    // The button that leads to the first or the last page is disabled once
    // it is there, and a disabled button drops the focus to nowhere. The
    // focus then goes to the text, which is what was asked for.
    focusText.current = next <= 0 || next >= page.pages - 1;
    setAt(next);
  };
  useEffect(() => {
    if (!focusText.current) return;
    focusText.current = false;
    viewer.current?.querySelector('pre')?.focus();
  }, [page.page]);

  return (
    <section className={styles.viewer} aria-label="Captured text">
      <div className={styles.viewerActions}>
        <span className={styles.viewerWhat}>
          {item.lines === undefined ? 'Text' : plural(item.lines, 'line')} as stored
        </span>
        <Button
          size="sm"
          variant="secondary"
          icon={Download}
          title="The stored text, byte for byte: its SHA-256 is the one this item states."
          onClick={() => download(evidenceFilename(item, 'txt'), text)}
        >
          Download as stored
        </Button>
      </div>
      {text === '' ? (
        <p className={styles.unknownBody} role="status">
          The item is empty: nothing was printed in what was captured. That is what was read, not a
          failure to read.
        </p>
      ) : (
        <>
          {page.pages > 1 && (
            <div className={styles.textPages} role="group" aria-label="Lines on screen">
              <span data-testid="evidence-lines-shown">
                Lines {formatCount(page.firstLine)} to {formatCount(page.lastLine)} of{' '}
                {formatCount(page.totalLines)} are on screen. The download is all of them.
              </span>
              <span className={styles.textPageButtons}>
                <IconButton
                  icon={ChevronsLeft}
                  size="sm"
                  aria-label="The first lines"
                  disabled={page.page === 0}
                  onClick={() => show(0)}
                />
                <IconButton
                  icon={ChevronLeft}
                  size="sm"
                  aria-label="Earlier lines"
                  disabled={page.page === 0}
                  onClick={() => show(page.page - 1)}
                />
                <IconButton
                  icon={ChevronRight}
                  size="sm"
                  aria-label="Later lines"
                  disabled={page.page === page.pages - 1}
                  onClick={() => show(page.page + 1)}
                />
                <IconButton
                  icon={ChevronsRight}
                  size="sm"
                  aria-label="The last lines"
                  disabled={page.page === page.pages - 1}
                  onClick={() => show(page.pages - 1)}
                />
              </span>
            </div>
          )}
          <div data-testid="evidence-text" ref={viewer}>
            {/* The copy button copies what is on screen: the pictures, not the control characters.
                Keyed by the page so a new one starts at its top, not where the last was scrolled to. */}
            <CodeBlock
              key={page.page}
              code={page.text}
              lineNumbers
              firstLine={page.firstLine}
              maxHeight="62vh"
            />
          </div>
        </>
      )}
      <p className={styles.viewerNote}>
        {pictured > 0 && (
          <span data-testid="evidence-pictured">
            {plural(pictured, 'control character')} in it {pictured === 1 ? 'is' : 'are'} shown as{' '}
            {pictured === 1 ? 'a picture of itself' : 'pictures of themselves'} (␛ is an escape). They are
            part of what was printed, and nothing here obeys them.{' '}
          </span>
        )}
        Secrets were replaced before it was stored; <code>&lt;hidden&gt;</code> and{' '}
        <code>&lt;secret:N&gt;</code> stand where they were.
      </p>
    </section>
  );
}

function DrawingBody({ item, svg }: { item: EvidenceWithBody; svg: string }) {
  // The one place the stored string becomes something a browser renders: an
  // address for an <img>. See `svgImageSrc` for why that is safe.
  const src = useMemo(() => svgImageSrc(svg), [svg]);
  const [broken, setBroken] = useState(false);
  const name = evidenceFilename(item, 'png');

  return (
    <section className={styles.viewer} aria-label="Captured drawing">
      <div className={styles.viewerActions}>
        <span className={styles.viewerWhat}>The engine’s drawing, as stored</span>
        <Button
          size="sm"
          variant="secondary"
          icon={Copy}
          disabled={broken}
          onClick={() => void copyImageOrSave(svgToPng(svg), name).then(tellImageCopied)}
        >
          Copy image
        </Button>
        <Button
          size="sm"
          variant="secondary"
          icon={ImageDown}
          disabled={broken}
          onClick={() => void savePicture(svgToPng(svg), name).then(tellSaved)}
        >
          Save as PNG
        </Button>
        <Button
          size="sm"
          variant="secondary"
          icon={Download}
          title="The stored SVG, byte for byte: its SHA-256 is the one this item states."
          // As plain text on purpose: see the note at the top of this file.
          onClick={() => download(evidenceFilename(item, 'svg'), svg)}
        >
          Download SVG
        </Button>
      </div>
      <div className={styles.drawing}>
        {broken ? (
          <p className={styles.unknownBody} role="alert">
            What is stored cannot be shown as a picture: this window does not read it as an SVG image.
            It is kept exactly as it was received and can still be downloaded.
          </p>
        ) : (
          <img
            className={styles.drawingImage}
            data-testid="evidence-drawing"
            src={src}
            alt={`Drawing: ${item.title}`}
            decoding="async"
            draggable={false}
            onError={() => setBroken(true)}
          />
        )}
      </div>
      <p className={styles.viewerNote}>
        Shown as an image, so nothing inside it can run or fetch anything. It is the engine’s drawing of
        the graph as it was when this was captured — not a picture of anybody’s screen.
      </p>
    </section>
  );
}

function ChartBody({
  item,
  body,
}: {
  item: EvidenceWithBody;
  body: Extract<EvidenceBody, { type: 'chart' }>;
}) {
  const chart = useMemo(() => storedChart(body), [body]);
  const lines = useMemo(() => coverageLines(body.coverage), [body.coverage]);
  const gaps = gapSentence(chart);
  const period = `${formatDateTime(body.coverage.since)} to ${formatDateTime(body.coverage.until)}`;

  return (
    <section className={styles.viewer} aria-label="Captured chart">
      <div className={styles.chartCard}>
        <TimeSeriesChart
          title={item.title}
          subtitle={period}
          timestamps={chart.timestamps}
          series={chart.series}
          height={260}
          exportMenu={{
            filename: `${item.title} ${item.id}`,
            // The picture leaves the gaps as gaps; the note says they are gaps.
            note: gaps ?? `${plural(body.coverage.points, 'stored point')}, ${period}`,
          }}
        />
        {gaps && (
          <p className={styles.gaps} data-testid="evidence-gaps">
            <Icon icon={Unplug} size={12} />
            {gaps}
          </p>
        )}
      </div>

      <div className={styles.coverage} data-testid="evidence-coverage">
        <h3 className={styles.coverageTitle}>Coverage, as the engine recorded it</h3>
        <p className={styles.viewerNote}>
          The period asked for was {period}. {plural(body.coverage.points, 'point')} over{' '}
          {plural(body.coverage.series.length, 'series', 'series')} were stored, exactly as they were
          read: a time without a sample has no point, and none was filled in.
        </p>
        <ul className={styles.coverageList}>
          {lines.map((line, index) => (
            <li key={index} data-empty={line.empty || undefined}>
              <span className={styles.coverageName}>
                {line.name}
                {line.lab && <LabPill />}
              </span>
              <span className={line.empty ? styles.coverageEmpty : undefined}>{line.text}</span>
            </li>
          ))}
        </ul>
        {chart.unreadable > 0 && (
          <p className={styles.viewerWarn} role="status">
            {plural(chart.unreadable, 'series', 'series')} in this item {chart.unreadable === 1 ? 'has' : 'have'}{' '}
            no readable points and {chart.unreadable === 1 ? 'is' : 'are'} drawn empty. What is stored is
            unchanged.
          </p>
        )}
        {chart.dropped > 0 && (
          <p className={styles.viewerWarn} role="status">
            {formatCount(chart.dropped)} stored {chart.dropped === 1 ? 'point is' : 'points are'} not a time
            and a number, and {chart.dropped === 1 ? 'is' : 'are'} not drawn.
          </p>
        )}
      </div>
    </section>
  );
}
