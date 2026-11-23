// @vitest-environment happy-dom
/**
 * Pictures and numbers out of the interface.
 *
 * A DOM without a browser has no canvas, so what is tested here is what does
 * not need one — the CSV, the layout, the limits, which of clipboard and
 * download a "Copy image" ends in — and, with the platform stood in for, the
 * path an SVG takes to a picture: as an image, never as a document.
 * `tests/ops.spec.ts` makes a real PNG of a real drawing and a real chart in
 * Chromium.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { toast } from '@/design/Toast/toasts';
import {
  CaptureError,
  MAX_PIXELS,
  MAX_SIDE,
  canvasSize,
  chartCsv,
  chartLayout,
  copyImageOrSave,
  csvCell,
  fileStem,
  savePicture,
  svgDataUrl,
  svgToPng,
  tellImageCopied,
} from './capture';

const T0 = Date.UTC(2026, 10, 22, 21, 40, 0);

describe('the numbers behind a chart, as CSV', () => {
  it('writes a gap as an empty cell and a zero as a zero', () => {
    const csv = chartCsv({
      timestamps: [T0, T0 + 60_000, T0 + 120_000],
      series: [
        { label: 'CPU', values: [2, null, 0] },
        { label: 'Memory', values: [41.5, 42, null] },
      ],
    });
    expect(csv).toBe(
      [
        'time_utc,CPU,Memory',
        '2026-11-22T21:40:00.000Z,2,41.5',
        '2026-11-22T21:41:00.000Z,,42',
        '2026-11-22T21:42:00.000Z,0,',
        '',
      ].join('\r\n'),
    );
    // The hole is not a number anywhere in the file.
    expect(csv.split('\r\n')[2]).not.toMatch(/,0|,null|,NaN/);
  });

  it('leaves out a time at which no series has a value: the chart put it there to lift its pen', () => {
    const csv = chartCsv({
      timestamps: [T0, T0 + 10_000, T0 + 600_000],
      series: [{ label: 'CPU', values: [2, null, 3] }],
    });
    expect(csv.trimEnd().split('\r\n')).toEqual([
      'time_utc,CPU',
      '2026-11-22T21:40:00.000Z,2',
      '2026-11-22T21:50:00.000Z,3',
    ]);
  });

  it('does not write a value that is not a number as one', () => {
    const csv = chartCsv({
      timestamps: [T0, T0 + 1, T0 + 2, T0 + 3],
      series: [
        { label: 'a', values: [Number.NaN, Number.POSITIVE_INFINITY, undefined, 1] },
        { label: 'b', values: [1, 1, 1, 1] },
      ],
    });
    expect(csv.trimEnd().split('\r\n').slice(1).map((row) => row.split(',')[1])).toEqual(['', '', '', '1']);
  });

  it('keeps full precision and a negative number as a number', () => {
    const csv = chartCsv({ timestamps: [T0], series: [{ label: 'dBm', values: [-17.123456789] }] });
    expect(csv).toContain(',-17.123456789\r\n');
  });

  it('quotes out a series label a spreadsheet would run as a formula', () => {
    const csv = chartCsv({
      timestamps: [T0],
      series: [
        { label: '=HYPERLINK("http://evil.example","Gi0/1")', values: [1] },
        { label: '+cmd|calc', values: [1] },
        { label: '-2+3', values: [1] },
        { label: '@SUM(A1)', values: [1] },
      ],
    });
    const header = csv.split('\r\n')[0];
    expect(header).toBe(
      `time_utc,"'=HYPERLINK(""http://evil.example"",""Gi0/1"")",'+cmd|calc,'-2+3,'@SUM(A1)`,
    );
  });

  it('quotes a label with a comma, a quote or a line break, and nothing else', () => {
    expect(csvCell('Traffic in · Gi1/0/1')).toBe('Traffic in · Gi1/0/1');
    expect(csvCell('uplink, to core')).toBe('"uplink, to core"');
    expect(csvCell('say "hi"')).toBe('"say ""hi"""');
    expect(csvCell('two\nlines')).toBe('"two\nlines"');
    expect(csvCell('\t=1+1')).toBe(`'\t=1+1`);
  });
});

describe('the canvas a picture is drawn on', () => {
  it('is the drawing at the scale asked for when that fits', () => {
    expect(canvasSize(620, 288, 2)).toEqual({ width: 1240, height: 576, scale: 2 });
  });

  it('is shrunk, never stretched, for a drawing that claims to be enormous', () => {
    const size = canvasSize(1_000_000, 1_000_000, 2);
    expect(size.width).toBeLessThanOrEqual(MAX_SIDE);
    expect(size.height).toBeLessThanOrEqual(MAX_SIDE);
    expect(size.width * size.height).toBeLessThanOrEqual(MAX_PIXELS);
    expect(size.width).toBe(size.height);
    const wide = canvasSize(100_000, 100, 1);
    expect(wide.width).toBe(MAX_SIDE);
    expect(wide.height).toBe(8);
  });

  it('has a size even when the drawing does not say one', () => {
    expect(canvasSize(0, 0, 1)).toEqual({ width: 1280, height: 800, scale: 1 });
    expect(canvasSize(Number.NaN, -5, 0)).toEqual({ width: 1280, height: 800, scale: 1 });
  });
});

describe('the layout of a chart picture', () => {
  const measure = (text: string) => text.length * 6;

  it('puts the title above the plot and nothing else when there is one series', () => {
    const layout = chartLayout({}, { width: 600, height: 220 }, measure);
    expect(layout.legend).toEqual([]);
    expect(layout.subtitle).toBeNull();
    expect(layout.note).toBeNull();
    expect(layout.plot.y).toBeGreaterThan(layout.title.y);
    expect(layout.width).toBe(632);
    expect(layout.height).toBe(layout.plot.y + 220 + 16);
  });

  it('wraps the legend to the width of the plot instead of running off the picture', () => {
    const legend = Array.from({ length: 8 }, (_unused, index) => ({
      label: `Traffic in · GigabitEthernet1/0/${index + 1}`,
      color: '#000',
    }));
    const layout = chartLayout({ legend }, { width: 600, height: 220 }, measure);
    const rows = new Set(layout.legend.map((key) => key.y));
    expect(layout.legend).toHaveLength(8);
    expect(rows.size).toBeGreaterThan(1);
    for (const key of layout.legend) {
      expect(key.x + 12 + 6 + measure(key.label)).toBeLessThanOrEqual(layout.width - 16);
    }
    // The plot starts under the last legend row.
    expect(layout.plot.y).toBeGreaterThan(Math.max(...rows));
  });

  it('makes room under the plot for the note that says what the picture leaves out', () => {
    const without = chartLayout({}, { width: 600, height: 220 }, measure);
    const withNote = chartLayout({ note: '2 gaps, 14 min in all' }, { width: 600, height: 220 }, measure);
    expect(withNote.note?.y).toBeGreaterThan(withNote.plot.y + 220);
    expect(withNote.height).toBeGreaterThan(without.height);
  });
});

describe('a file name made from a title', () => {
  it('is lower case, hyphenated and never empty', () => {
    expect(fileStem('CPU · Ops/SW1')).toBe('cpu-ops-sw1');
    expect(fileStem('../../etc/passwd')).toBe('etc-passwd');
    expect(fileStem('···')).toBe('chart');
    expect(fileStem('', 'topology')).toBe('topology');
  });
});

// ------------------------------------------------- with the platform stood in

/** A stand-in for `Image`: it "loads" whatever it is pointed at, and records it. */
class FakeImage {
  static loaded: string[] = [];
  static size = { width: 620, height: 288 };
  static fails = false;
  decoding = '';
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  naturalWidth = 0;
  naturalHeight = 0;
  set src(value: string) {
    FakeImage.loaded.push(value);
    queueMicrotask(() => {
      if (FakeImage.fails) {
        this.onerror?.();
        return;
      }
      this.naturalWidth = FakeImage.size.width;
      this.naturalHeight = FakeImage.size.height;
      this.onload?.();
    });
  }
}

const platform: {
  blobs: Blob[];
  revoked: string[];
  drawn: unknown[];
  canvases: { width: number; height: number }[];
  toBlob: (done: (blob: Blob | null) => void) => void;
} = {
  blobs: [],
  revoked: [],
  drawn: [],
  canvases: [],
  toBlob: (done) => done(new Blob([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], { type: 'image/png' })),
};

beforeEach(() => {
  platform.blobs = [];
  platform.revoked = [];
  platform.drawn = [];
  platform.canvases = [];
  platform.toBlob = (done) => done(new Blob([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], { type: 'image/png' }));
  FakeImage.loaded = [];
  FakeImage.size = { width: 620, height: 288 };
  FakeImage.fails = false;
  vi.stubGlobal('Image', FakeImage);
  vi.spyOn(URL, 'createObjectURL').mockImplementation((blob) => {
    platform.blobs.push(blob as Blob);
    return `blob:test/${platform.blobs.length}`;
  });
  vi.spyOn(URL, 'revokeObjectURL').mockImplementation((url) => {
    platform.revoked.push(url);
  });
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(function (this: HTMLCanvasElement) {
    const canvas = this;
    return {
      fillStyle: '',
      fillRect: () => undefined,
      drawImage: (image: unknown) => {
        platform.drawn.push(image);
        platform.canvases.push({ width: canvas.width, height: canvas.height });
      },
    } as unknown as CanvasRenderingContext2D;
  } as never);
  vi.spyOn(HTMLCanvasElement.prototype, 'toBlob').mockImplementation((done) => platform.toBlob(done));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const HOSTILE =
  '<svg xmlns="http://www.w3.org/2000/svg" width="620" height="288" onload="alert(1)"><script>alert(2)</script><rect width="10" height="10"/></svg>';

/** The text a `data:image/svg+xml;base64,` address holds. */
function decoded(address: string): string {
  const prefix = 'data:image/svg+xml;base64,';
  expect(address.startsWith(prefix)).toBe(true);
  return new TextDecoder().decode(Uint8Array.from(atob(address.slice(prefix.length)), (ch) => ch.charCodeAt(0)));
}

describe('the address a drawing is shown from', () => {
  it('holds exactly the drawing, encoded so that nothing in it can end the address', () => {
    const address = svgDataUrl(HOSTILE);
    expect(decoded(address)).toBe(HOSTILE);
    // Nothing of the markup is in the address as text.
    expect(address).not.toMatch(/[<>"' ]/);
    expect(address).not.toContain('script');
    expect(address).not.toContain('onload');
  });

  it('survives text outside Latin-1 and a lone surrogate, which a naive encoder throws on', () => {
    const svg = '<svg xmlns="http://www.w3.org/2000/svg"><text>核心交换机 — Αθήνα \ud83d\udd25 \ud800</text></svg>';
    expect(() => svgDataUrl(svg)).not.toThrow();
    expect(decoded(svgDataUrl(svg))).toBe(svg.replace('\ud800', '\ufffd'));
  });

  it('does not choke on a drawing of the largest size an item holds', () => {
    const big = `<svg xmlns="http://www.w3.org/2000/svg">${'<rect width="1" height="1"/>'.repeat(74_000)}</svg>`;
    expect(big.length).toBeGreaterThan(2_000_000);
    expect(decoded(svgDataUrl(big))).toBe(big);
  });
});

describe('an SVG on its way to a PNG', () => {
  it('is decoded as an image, from a data address that holds exactly its own text', async () => {
    const png = await svgToPng(HOSTILE);

    expect(png.type).toBe('image/png');
    // The only thing that was ever pointed at the drawing is an image.
    expect(FakeImage.loaded).toHaveLength(1);
    expect(decoded(FakeImage.loaded[0] ?? '')).toBe(HOSTILE);
    expect(platform.drawn).toHaveLength(1);
    expect(platform.drawn[0]).toBeInstanceOf(FakeImage);
    // No address was made that could be opened as a document of this origin.
    expect(platform.blobs).toEqual([]);
    // And nothing of the drawing became part of this document.
    expect(document.querySelector('svg, script, iframe, object, embed')).toBeNull();
    expect(document.body.innerHTML).not.toContain('alert');
  });

  it('is drawn at twice its size by default, and no bigger than the limits whatever it claims', async () => {
    await svgToPng(HOSTILE);
    expect(platform.canvases[0]).toEqual({ width: 1240, height: 576 });

    FakeImage.size = { width: 1_000_000, height: 1_000_000 };
    await svgToPng(HOSTILE);
    const huge = platform.canvases[1];
    expect(huge?.width).toBeLessThanOrEqual(MAX_SIDE);
    expect((huge?.width ?? 0) * (huge?.height ?? 0)).toBeLessThanOrEqual(MAX_PIXELS);
  });

  it('says so when the text is not a drawing', async () => {
    FakeImage.fails = true;
    await expect(svgToPng('not an svg')).rejects.toBeInstanceOf(CaptureError);
    expect(platform.drawn).toEqual([]);
  });

  it('says so when the window refuses to read the canvas back, and points at the SVG', async () => {
    platform.toBlob = () => {
      throw new DOMException('The canvas has been tainted by cross-origin data.', 'SecurityError');
    };
    await expect(svgToPng(HOSTILE)).rejects.toThrow(/Save the SVG instead/);
  });

  it('says so when the encoder gives nothing back', async () => {
    platform.toBlob = (done) => done(null);
    await expect(svgToPng(HOSTILE)).rejects.toThrow('could not be encoded');
  });
});

describe('copy image', () => {
  const picture = () => Promise.resolve(new Blob(['png'], { type: 'image/png' }));

  /** A clipboard that takes pictures, and remembers what it was given. */
  function clipboard(write: (items: unknown[]) => Promise<void>): { items: unknown[][] } {
    const seen: { items: unknown[][] } = { items: [] };
    class Item {
      constructor(readonly data: Record<string, unknown>) {}
    }
    vi.stubGlobal('ClipboardItem', Item);
    vi.stubGlobal('navigator', {
      clipboard: {
        write: (items: unknown[]) => {
          seen.items.push(items);
          return write(items);
        },
      },
    });
    return seen;
  }

  it('puts the picture on the clipboard as a PNG, handed over as a promise so WebKit still takes it', async () => {
    const seen = clipboard(async () => undefined);
    const made = picture();
    const outcome = await copyImageOrSave(made, 'map.png');

    expect(outcome).toEqual({ copied: true });
    const item = seen.items[0]?.[0] as { data: Record<string, unknown> };
    expect(Object.keys(item.data)).toEqual(['image/png']);
    expect(item.data['image/png']).toBe(made);
    // Nothing was downloaded as well.
    expect(platform.blobs).toEqual([]);
  });

  it('saves the picture instead where the clipboard refuses an image, and says that is what happened', async () => {
    clipboard(async () => {
      throw new DOMException('Write permission denied.', 'NotAllowedError');
    });
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined);
    const outcome = await copyImageOrSave(picture(), 'map.png');

    expect(outcome).toEqual({
      copied: false,
      saved: { ok: true, filename: 'map.png', confirmed: false, via: 'browser' },
    });
    expect(click).toHaveBeenCalledTimes(1);
    expect(platform.blobs[0]?.type).toBe('image/png');

    const success = vi.spyOn(toast, 'success');
    const info = vi.spyOn(toast, 'info');
    tellImageCopied(outcome);
    expect(success).not.toHaveBeenCalled();
    expect(info.mock.calls[0]?.[0]).toBe('Saved as map.png instead');
  });

  it('saves it where the window has no image clipboard at all', async () => {
    vi.stubGlobal('navigator', { clipboard: { writeText: async () => undefined } });
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined);
    const outcome = await copyImageOrSave(picture(), 'chart.png');
    expect(outcome.copied).toBe(false);
    expect(outcome.copied === false && outcome.saved.ok).toBe(true);
  });

  it('says the picture could not be made, rather than copying or saving nothing', async () => {
    clipboard(async (items) => {
      await (items[0] as { data: Record<string, Promise<Blob>> }).data['image/png'];
    });
    const failed = Promise.reject(new CaptureError('The chart has not been drawn yet, so there is no picture of it.'));
    const outcome = await copyImageOrSave(failed, 'chart.png');

    expect(outcome).toEqual({
      copied: false,
      saved: {
        ok: false,
        filename: 'chart.png',
        reason: 'The chart has not been drawn yet, so there is no picture of it.',
      },
    });
    expect(platform.blobs).toEqual([]);

    const failure = vi.spyOn(toast, 'error');
    const success = vi.spyOn(toast, 'success');
    tellImageCopied(outcome);
    expect(success).not.toHaveBeenCalled();
    expect(failure.mock.calls[0]?.[0]).toBe('The image was neither copied nor saved');
  });
});

describe('save as PNG', () => {
  it('reports a picture that could not be made as a failed save, with the reason', async () => {
    const outcome = await savePicture(Promise.reject(new CaptureError('no canvas')), 'map.png');
    expect(outcome).toEqual({ ok: false, filename: 'map.png', reason: 'no canvas' });
  });
});
