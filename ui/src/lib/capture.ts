/**
 * Pictures and numbers out of the interface, without storing either.
 *
 * Four things, all of them made in the page from what the page already holds:
 *
 * - the engine's own SVG of the map, as a PNG ({@link svgToPng});
 * - a chart — its plot canvas with the title and the legend painted around it,
 *   because those are HTML beside the canvas and a picture of the plot alone
 *   says nothing about what it plots ({@link chartToPng});
 * - "Copy image", with a download as the fallback where the clipboard refuses
 *   an image, which a WebView may ({@link copyImageOrSave});
 * - the numbers behind a chart as CSV ({@link chartCsv}).
 *
 * What this module deliberately is not: a screenshot. It never reads pixels
 * of the screen, a window or the terminal (`docs/dev/spec-capture.md`, "Not
 * building"): a picture of `show running-config` is a secret nobody can find
 * again. A drawing comes from the engine's SVG and a chart from its own
 * canvas; evidence that is to be *kept* is captured by the engine
 * (`evidence.capture_topology`, `evidence.capture_chart`), not uploaded from
 * here.
 *
 * An SVG handed to {@link svgToPng} may be hostile — a stored drawing is not
 * sanitised by design. It is only ever decoded as an image (`Image` on a
 * `data:` address), where a browser runs no script and fetches nothing, and
 * the address is never navigated to, framed or opened.
 */

import { saveFile, type SaveOutcome } from './download';
import { toast } from '@/design/Toast/toasts';

/** A picture could not be made. `message` is a sentence for the person who asked. */
export class CaptureError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CaptureError';
  }
}

/** The longest side of a picture, in device pixels: every engine still makes a canvas this big. */
export const MAX_SIDE = 8192;
/** And the most pixels: a drawing that claims a million by a million is not given the memory. */
export const MAX_PIXELS = 32_000_000;
/** What an SVG without a size of its own is drawn at. */
const FALLBACK_SIZE = { width: 1280, height: 800 };
const UI_FONT = "'Inter Variable', system-ui, sans-serif";

function reason(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

/**
 * The size of the canvas for a picture of `width` by `height` CSS pixels at
 * `scale`, shrunk (never stretched) until it is inside both limits.
 */
export function canvasSize(
  width: number,
  height: number,
  scale: number,
): { width: number; height: number; scale: number } {
  const w = Number.isFinite(width) && width > 0 ? width : FALLBACK_SIZE.width;
  const h = Number.isFinite(height) && height > 0 ? height : FALLBACK_SIZE.height;
  let factor = Number.isFinite(scale) && scale > 0 ? scale : 1;
  factor = Math.min(factor, MAX_SIDE / w, MAX_SIDE / h, Math.sqrt(MAX_PIXELS / (w * h)));
  return {
    width: Math.max(1, Math.floor(w * factor)),
    height: Math.max(1, Math.floor(h * factor)),
    scale: factor,
  };
}

function toPng(canvas: HTMLCanvasElement): Promise<Blob> {
  return new Promise((resolve, reject) => {
    try {
      canvas.toBlob((blob) => {
        if (blob) resolve(blob);
        else reject(new CaptureError('The picture could not be encoded as a PNG.'));
      }, 'image/png');
    } catch (error) {
      // A canvas that something cross-origin was drawn on (an SVG with a
      // `foreignObject`, on WebKit) refuses to be read back.
      reject(
        new CaptureError(
          `This window will not turn that drawing into a PNG (${reason(error, 'the canvas cannot be read')}). Save the SVG instead.`,
        ),
      );
    }
  });
}

function context(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  const found = canvas.getContext('2d');
  if (!found) throw new CaptureError('This window cannot draw a picture: it has no 2D canvas.');
  return found;
}

function base64(bytes: Uint8Array): string {
  let binary = '';
  // In pieces: a two-megabyte drawing is more arguments than a call takes.
  for (let at = 0; at < bytes.length; at += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
  }
  return btoa(binary);
}

/**
 * The address an `<img>` shows an SVG from: the SVG's own bytes, base64,
 * behind `data:image/svg+xml`.
 *
 * This is how a drawing that may be hostile reaches the screen, so it is
 * worth saying why it holds. An SVG loaded *as an image* is rendered in a
 * mode where scripts do not run, event handlers are not attached and no
 * external resource is fetched; whatever the string contains, the `<img>`
 * shows a picture or nothing. The string is encoded, never interpolated, so
 * it cannot end the attribute or the address.
 *
 * `data:` rather than `blob:` because of what happens if the address ever
 * escapes the `<img>` — "open image in new tab" is one click. A `blob:`
 * address made here has this application's origin, and a drawing opened as a
 * document on it would run its script with this application's storage in
 * reach. A `data:` document has an origin of its own and reaches nothing.
 */
export function svgDataUrl(svg: string): string {
  // TextEncoder writes a lone surrogate as U+FFFD instead of throwing, so
  // there is no string this fails on.
  return `data:image/svg+xml;base64,${base64(new TextEncoder().encode(svg))}`;
}

export interface SvgToPngOptions {
  /** Device pixels per SVG pixel. Default 2, so the picture survives a slide. */
  scale?: number;
  /** Painted under the drawing. Default white: the engine's drawing brings its own, a stored one may not. */
  background?: string;
}

/**
 * An SVG document as a PNG.
 *
 * The SVG is decoded as an image — from the same `data:` address an `<img>`
 * would show it from ({@link svgDataUrl}) — and drawn onto a canvas. Nothing
 * in it can run or load on that path, and no address is made that could be
 * opened as a document.
 */
export async function svgToPng(svg: string, { scale = 2, background = '#ffffff' }: SvgToPngOptions = {}): Promise<Blob> {
  if (typeof document === 'undefined' || typeof Image === 'undefined') {
    throw new CaptureError('This window cannot draw a picture.');
  }
  const image = new Image();
  image.decoding = 'async';
  await new Promise<void>((resolve, reject) => {
    image.onload = () => resolve();
    image.onerror = () =>
      reject(new CaptureError('The drawing is not an SVG this window can read, so there is no picture of it.'));
    image.src = svgDataUrl(svg);
  });
  const size = canvasSize(image.naturalWidth, image.naturalHeight, scale);
  const canvas = document.createElement('canvas');
  canvas.width = size.width;
  canvas.height = size.height;
  const pen = context(canvas);
  pen.fillStyle = background;
  pen.fillRect(0, 0, size.width, size.height);
  pen.drawImage(image, 0, 0, size.width, size.height);
  return toPng(canvas);
}

// ------------------------------------------------------------------- a chart

export interface LegendKey {
  label: string;
  /** Any CSS colour: the one the line is drawn in. */
  color: string;
}

export interface ChartPicture {
  /** The plot's own canvas, in device pixels. Drawn as it is, never re-plotted. */
  plot: HTMLCanvasElement;
  title: string;
  subtitle?: string;
  /** The series that are drawn. Left out for a single series: the title is its name. */
  legend?: readonly LegendKey[];
  /** A line under the plot: what the picture does not show (gaps, a range). */
  note?: string;
  colors: {
    /** The surface the chart sits on: the plot canvas is transparent. */
    background: string;
    text: string;
    muted: string;
  };
}

const PAD = 16;
const TITLE = { size: 14, line: 20 };
const SMALL = { size: 12, line: 18 };
const KEY = { width: 12, height: 3, gap: 6, after: 16 };

export interface ChartLayout {
  width: number;
  height: number;
  title: { x: number; y: number };
  subtitle: { x: number; y: number } | null;
  /** One entry per legend key, in order, wrapped to the width. */
  legend: { x: number; y: number; label: string; color: string }[];
  plot: { x: number; y: number; width: number; height: number };
  note: { x: number; y: number } | null;
}

/**
 * Where everything in a chart picture goes, in CSS pixels. Separate from the
 * drawing so that it can be checked without a canvas: `measure` is the width
 * of a legend label in the small font.
 */
export function chartLayout(
  picture: Pick<ChartPicture, 'subtitle' | 'legend' | 'note'>,
  plot: { width: number; height: number },
  measure: (text: string) => number,
): ChartLayout {
  const width = Math.max(plot.width, 240) + PAD * 2;
  let y = PAD;
  const title = { x: PAD, y: y + TITLE.line / 2 };
  y += TITLE.line;
  let subtitle: ChartLayout['subtitle'] = null;
  if (picture.subtitle) {
    subtitle = { x: PAD, y: y + SMALL.line / 2 };
    y += SMALL.line;
  }

  const legend: ChartLayout['legend'] = [];
  if (picture.legend && picture.legend.length > 0) {
    y += 6;
    let x = PAD;
    for (const key of picture.legend) {
      const wide = KEY.width + KEY.gap + Math.ceil(measure(key.label));
      // A key that does not fit goes to the next row whole; one wider than
      // the picture starts a row and is clipped by the edge, not dropped.
      if (x > PAD && x + wide > width - PAD) {
        x = PAD;
        y += SMALL.line;
      }
      legend.push({ x, y: y + SMALL.line / 2, label: key.label, color: key.color });
      x += wide + KEY.after;
    }
    y += SMALL.line;
  }

  y += 8;
  const placed = { x: PAD, y, width: plot.width, height: plot.height };
  y += plot.height;
  let note: ChartLayout['note'] = null;
  if (picture.note) {
    y += 6;
    note = { x: PAD, y: y + SMALL.line / 2 };
    y += SMALL.line;
  }
  return { width, height: y + PAD, title, subtitle, legend, plot: placed, note };
}

/**
 * A chart as a PNG: the plot canvas as it is on screen, with its title, its
 * legend and an optional note painted around it on the chart's own surface
 * colour. Whatever theme is on screen is the theme of the picture.
 */
export async function chartToPng(picture: ChartPicture): Promise<Blob> {
  if (typeof document === 'undefined') throw new CaptureError('This window cannot draw a picture.');
  const { plot } = picture;
  if (plot.width === 0 || plot.height === 0) {
    throw new CaptureError('The chart has not been drawn yet, so there is no picture of it.');
  }
  // The plot is in device pixels; everything else is laid out in CSS pixels
  // and drawn at the same ratio, so the text is as sharp as the lines.
  const shown = plot.clientWidth > 0 ? plot.clientWidth : plot.width / (window.devicePixelRatio || 1);
  const ratio = plot.width / shown;
  const css = { width: shown, height: plot.height / ratio };

  const canvas = document.createElement('canvas');
  const pen = context(canvas);
  const small = `${SMALL.size}px ${UI_FONT}`;
  pen.font = small;
  const layout = chartLayout(picture, css, (text) => pen.measureText(text).width);
  const size = canvasSize(layout.width, layout.height, ratio);
  canvas.width = size.width;
  canvas.height = size.height;

  // Resizing a canvas resets its state, so everything is set from here on.
  pen.scale(size.scale, size.scale);
  pen.fillStyle = picture.colors.background;
  pen.fillRect(0, 0, layout.width, layout.height);
  pen.textBaseline = 'middle';

  pen.fillStyle = picture.colors.text;
  pen.font = `600 ${TITLE.size}px ${UI_FONT}`;
  pen.fillText(picture.title, layout.title.x, layout.title.y, layout.width - PAD * 2);

  pen.font = small;
  if (layout.subtitle && picture.subtitle) {
    pen.fillStyle = picture.colors.muted;
    pen.fillText(picture.subtitle, layout.subtitle.x, layout.subtitle.y, layout.width - PAD * 2);
  }
  for (const key of layout.legend) {
    pen.fillStyle = key.color;
    pen.fillRect(key.x, key.y - KEY.height / 2, KEY.width, KEY.height);
    pen.fillStyle = picture.colors.text;
    pen.fillText(key.label, key.x + KEY.width + KEY.gap, key.y, layout.width - PAD - key.x);
  }
  pen.drawImage(plot, layout.plot.x, layout.plot.y, layout.plot.width, layout.plot.height);
  if (layout.note && picture.note) {
    pen.fillStyle = picture.colors.muted;
    pen.fillText(picture.note, layout.note.x, layout.note.y, layout.width - PAD * 2);
  }
  return toPng(canvas);
}

// ------------------------------------------------------------- the clipboard

export type ImageCopyOutcome =
  /** The picture is on the clipboard. */
  | { copied: true }
  /** The clipboard would not take a picture; this is what became of saving it instead. */
  | { copied: false; saved: SaveOutcome };

function clipboardTakesImages(): boolean {
  return (
    typeof navigator !== 'undefined' &&
    typeof navigator.clipboard?.write === 'function' &&
    typeof ClipboardItem === 'function'
  );
}

/**
 * Puts a picture on the clipboard, or — where the platform refuses an image
 * there, as a WebView may, or a page that is not on a secure address — saves
 * it as `filename` instead, and says which of the two happened.
 *
 * It takes the *promise* of the picture, not the picture: WebKit only lets a
 * page write to the clipboard inside the click that asked for it, and making
 * a PNG takes longer than that. A `ClipboardItem` built from a promise is
 * handed over in time and filled in when the picture is ready.
 */
export async function copyImageOrSave(picture: Promise<Blob>, filename: string): Promise<ImageCopyOutcome> {
  if (clipboardTakesImages()) {
    try {
      await navigator.clipboard.write([new ClipboardItem({ 'image/png': picture })]);
      return { copied: true };
    } catch {
      // Refused, or the picture itself failed: the next step tells which.
    }
  }
  let blob: Blob;
  try {
    blob = await picture;
  } catch (error) {
    return {
      copied: false,
      saved: { ok: false, filename, reason: reason(error, 'The picture could not be made.') },
    };
  }
  return { copied: false, saved: saveFile(filename, blob, 'image/png') };
}

/** Says what became of a "Copy image", in a toast. */
export function tellImageCopied(outcome: ImageCopyOutcome): void {
  if (outcome.copied) {
    toast.success('Image copied');
    return;
  }
  const { saved } = outcome;
  if (!saved.ok) {
    toast.error('The image was neither copied nor saved', { description: saved.reason });
    return;
  }
  toast.info(`Saved as ${saved.filename} instead`, {
    description:
      saved.via === 'shell'
        ? 'This window does not put a picture on the clipboard, so the system was asked to save it. If no file appears, save the SVG or the CSV instead.'
        : 'This window does not put a picture on the clipboard, so it was downloaded.',
  });
}

/** Saves a picture and says so; a picture that could not be made is said too. */
export async function savePicture(picture: Promise<Blob>, filename: string): Promise<SaveOutcome> {
  try {
    return saveFile(filename, await picture, 'image/png');
  } catch (error) {
    return { ok: false, filename, reason: reason(error, 'The picture could not be made.') };
  }
}

// ---------------------------------------------------------------------- CSV

export interface ChartTable {
  /** Sample times in milliseconds since the epoch. */
  timestamps: readonly number[];
  /** One value per timestamp. `null` is a gap: nothing was recorded there. */
  series: readonly { label: string; values: readonly (number | null | undefined)[] }[];
}

/**
 * One cell of text. A spreadsheet takes a cell that starts with `=`, `+`, `-`
 * or `@` (or a tab or a carriage return in front of one) for a formula, and a
 * series label can be an interface description a stranger typed. Such a cell
 * gets an apostrophe in front, as the engine's own CSV does.
 */
export function csvCell(text: string): string {
  const guarded = /^[=+\-@\t\r]/.test(text) ? `'${text}` : text;
  return /[",\r\n]/.test(guarded) ? `"${guarded.replaceAll('"', '""')}"` : guarded;
}

/**
 * The numbers behind a chart, one row per sample time.
 *
 * - Time is ISO 8601 in UTC, which sorts as text and which no locale reads
 *   differently.
 * - A value is written in full, with a point for the decimal mark and no
 *   grouping: a file for a spreadsheet, not for reading.
 * - **A gap is an empty cell, never a zero.** "Nothing was recorded" and
 *   "zero was recorded" are different facts, and a zero would be averaged.
 * - A row in which no series has a value is not written at all: the chart
 *   puts a time there only to lift its pen, and no sample was taken at it.
 */
export function chartCsv(table: ChartTable): string {
  const rows: string[] = [['time_utc', ...table.series.map((entry) => csvCell(entry.label))].join(',')];
  table.timestamps.forEach((at, index) => {
    const cells = table.series.map((entry) => {
      const value = entry.values[index];
      return typeof value === 'number' && Number.isFinite(value) ? String(value) : '';
    });
    if (cells.every((cell) => cell === '')) return;
    const time = Number.isFinite(at) ? new Date(at) : null;
    rows.push([time && !Number.isNaN(time.getTime()) ? time.toISOString() : '', ...cells].join(','));
  });
  return `${rows.join('\r\n')}\r\n`;
}

/** A file name stem from a title: lower case, hyphens, nothing a shell minds. */
export function fileStem(title: string, fallback = 'chart'): string {
  const stem = title
    .normalize('NFKD')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 60)
    .replace(/-+$/, '');
  return stem === '' ? fallback : stem;
}
