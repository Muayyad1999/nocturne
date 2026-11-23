/**
 * Evidence, reasoned about.
 *
 * What these tests hold the screen to: text is shown as it was printed and
 * obeyed by nothing; a stored chart is drawn with its holes and its coverage
 * is said in words; a list says what it is a page of; and a deleted item's
 * tombstone is told apart from an item, from an event and from nothing.
 *
 * The item, list and timeline fixtures are real answers: `evidence.*` and
 * `incidents.get` of a `nocturne-server` with a lab switch (`Ops/SW1`) whose
 * terminal session, samples and topology were captured through the RPC
 * surface, and the entry and tombstone recorded by `nocturne-ops`' own test
 * run. Fixtures written by hand to reach a case no engine produced are marked
 * synthetic where they are defined.
 */

import { describe, expect, it } from 'vitest';
import { RpcError } from '@/api/errors';
import type { Evidence, EvidenceBody, EvidenceCoverage, Handover, TimelineEntry } from '@/api/types/ops';
import {
  attachedEvidenceId,
  coverageLines,
  deleteWarning,
  evidenceFilename,
  failure,
  gapSentence,
  isEvidenceKind,
  isOperatorSupplied,
  isTombstone,
  listCoverage,
  picturedCharacters,
  sourceSentence,
  storedChart,
  svgImageSrc,
  textLines,
  textPage,
  visibleText,
} from './evidenceReading';
import { ENTRY_WORD, RESOLVABLE_KINDS, entryLink, handoverSections, refLink } from './ops';

// ----------------------------------------------------- real answers, as fixtures

/** `evidence.list` on a real server: a terminal capture of a lab session. */
const TERMINAL: Evidence = {
  id: 'lks55ph2ugbxlatu',
  kind: 'terminal',
  title: 'Ops/SW1 — last 25 lines',
  captured_at: 1795386599842,
  source: {
    from: 'terminal.snapshot',
    session_id: '3czovjjxwgjsnnej',
    session_title: 'Ops/SW1',
    device_id: '7u5ygdhiejg4mfx3',
    device: 'Ops/SW1',
    peer: 'lab:Ops/SW1',
    lab: true,
    params: { lines: 30 },
  },
  redacted: true,
  lines: 25,
  bytes: 792,
  sha256: 'd7587f1d4594121612e8856701146834af3f510213c65a1dd8abf30943ac79e8',
  incident_ids: [],
};

/** `evidence.add_text`, from the backend's recorded test run. */
const OPERATOR: Evidence = {
  id: 'xjgc4vs5b45kt32b',
  kind: 'text',
  title: 'raw please',
  captured_at: 1795383631868,
  source: { from: 'operator', lab: false, note: 'text as sent by the client, redacted on arrival' },
  redacted: true,
  lines: 1,
  bytes: 24,
  sha256: '33df5eb42d97994f90a70765a4685f5e3b30291891c07a0770b549f9a8251f9f',
  incident_ids: [],
};

/** The body of a real `evidence.capture_chart` of two series of one lab device. */
const REAL_CHART: Extract<EvidenceBody, { type: 'chart' }> = {
  type: 'chart',
  series: [
    { device_id: '7u5ygdhiejg4mfx3', metric: 'cpu.pct', instance: '', points: [[1795386599000, 2.0]], step_s: 1, rollup: 'raw' },
    {
      device_id: '7u5ygdhiejg4mfx3',
      metric: 'if.oper',
      instance: 'FastEthernet0/1',
      points: [[1795386599000, 0.0]],
      step_s: 1,
      rollup: 'raw',
    },
  ],
  coverage: {
    since: 1795386021633,
    until: 1795386621633,
    points: 2,
    series: [
      {
        device_id: '7u5ygdhiejg4mfx3',
        device: 'Ops/SW1',
        lab: true,
        metric: 'cpu.pct',
        instance: '',
        points: 1,
        first_at: 1795386599000,
        last_at: 1795386599000,
        step_s: 1,
        rollup: 'raw',
      },
      {
        device_id: '7u5ygdhiejg4mfx3',
        device: 'Ops/SW1',
        lab: true,
        metric: 'if.oper',
        instance: 'FastEthernet0/1',
        points: 1,
        first_at: 1795386599000,
        last_at: 1795386599000,
        step_s: 1,
        rollup: 'raw',
      },
    ],
  },
};

/** The timeline entry of an attached item, as `incidents.get` returned it. */
const ATTACHED: TimelineEntry = {
  id: 'evidence:wqqeevuttw2g3hbj',
  ts: 1795383623697,
  kind: 'evidence',
  text: 'SW1 password 7 <secret:4> — last 12 lines',
  ref: { type: 'evidence', id: 'wqqeevuttw2g3hbj' },
  author: 'mo',
  source: {
    from: 'terminal.snapshot',
    evidence:
      'evidence `wqqeevuttw2g3hbj`: terminal session `SW1 password 7 <secret:4>` (lab), 12 lines, redacted, sha256 b2622775faf5bd16c27a441a72106f27e5cbbff67811031cf568492c4d3aef3d',
    gathered_at: 1795383623929,
  },
};

/** The same incident after `evidence.delete`: the entry stays, without a reference. */
const TOMBSTONE: TimelineEntry = {
  id: 'evidence:pi7olxcxb36xscef',
  ts: 1795383623719,
  kind: 'evidence',
  text: 'the end of it, enable secret <secret:1> — evidence deleted on 2026-11-22 (UTC)',
  author: 'zahra',
  source: {
    from: 'evidence.delete',
    evidence:
      'evidence `pi7olxcxb36xscef` was deleted; it was terminal session `SW1 password 7 <secret:2>` (lab), 3 lines, redacted, sha256 8f11468dcbb886c2c4948586af262b4abf786e68364ff3be22f67446bfe4618a',
    gathered_at: 1795383624019,
  },
};

// ----------------------------------------------------------------------- text

describe('text a device printed', () => {
  it('shows an escape sequence as a picture of itself instead of something to obey', () => {
    // What a device prints to clear the screen and recolour a line.
    const printed = 'SW1#\u001b[2J\u001b[31mreload\u001b[0m';
    const shown = visibleText(printed);
    expect(shown).toBe('SW1#\u241b[2J\u241b[31mreload\u241b[0m');
    expect(shown).not.toContain('\u001b');
    expect(picturedCharacters(printed)).toBe(3);
  });

  it('shows a backspace and a lone carriage return, which make a line read as another on a terminal', () => {
    // "no shutdown" overwritten on screen to read "shutdown".
    expect(visibleText('no shutdown\rshutdown   ')).toBe('no shutdown\u240dshutdown   ');
    expect(visibleText('rm -rf /\b\b\b\b\b\b\b\bls      ')).toBe('rm -rf /\u2408\u2408\u2408\u2408\u2408\u2408\u2408\u2408ls      ');
  });

  it('keeps line breaks and tabs, and ends a CR LF line once', () => {
    expect(visibleText('a\tb\r\nc\n')).toBe('a\tb\nc\n');
    expect(picturedCharacters('a\tb\r\nc\n')).toBe(0);
  });

  it('shows every other control character, the delete and the bell among them', () => {
    expect(visibleText('\u0000\u0007\u007f')).toBe('\u2400\u2407\u2421');
    // The eight-bit controls have no picture of their own.
    expect(visibleText('a\u009b31mb')).toBe('a\ufffd31mb');
  });

  it('shows a direction override, which would make a line read backwards', () => {
    // Displays as "permit any any" followed by reversed text on a real screen.
    const printed = 'deny ip any any \u202eyna yna timrep';
    const shown = visibleText(printed);
    expect(shown).toBe('deny ip any any \ufffdyna yna timrep');
    for (const override of ['\u202a', '\u202b', '\u202c', '\u202d', '\u202e', '\u2066', '\u2067', '\u2068', '\u2069', '\u200e', '\u200f', '\u061c']) {
      expect(visibleText(`a${override}b`), `U+${override.codePointAt(0)?.toString(16)}`).toBe('a\ufffdb');
    }
  });

  it('leaves ordinary text, other scripts and the redactor’s own placeholders exactly as they are', () => {
    const text = 'enable secret <secret:1>\n<hidden>\n核心交换机 Αθήνα \ud83d\udd25 — “quoted”';
    expect(visibleText(text)).toBe(text);
    expect(picturedCharacters(text)).toBe(0);
  });
});

// -------------------------------------------------------------------- a chart

const MINUTE = 60_000;
const T0 = Date.UTC(2026, 10, 22, 21, 0, 0);

/** Synthetic: one series sampled every minute, with twenty minutes nobody recorded. */
function holed(): Extract<EvidenceBody, { type: 'chart' }> {
  const times = [0, 1, 2, 3, 24, 25, 26].map((minute) => T0 + minute * MINUTE);
  return {
    type: 'chart',
    series: [
      { device_id: 'd1', metric: 'cpu.pct', instance: '', points: times.map((at, index) => [at, index * 10]), step_s: 60, rollup: 'raw' },
    ],
    coverage: {
      since: T0,
      until: T0 + 30 * MINUTE,
      points: times.length,
      series: [
        {
          device_id: 'd1',
          device: 'core-sw1',
          lab: false,
          metric: 'cpu.pct',
          instance: '',
          points: times.length,
          first_at: times[0],
          last_at: times.at(-1),
          longest_gap_ms: 21 * MINUTE,
          step_s: 60,
          rollup: 'raw',
        },
      ],
    },
  };
}

describe('a long text, a page at a time', () => {
  const lines = textLines('a\nb\nc\nd\ne\n');

  it('counts lines as the viewer numbers them: a final line break does not start one', () => {
    expect(lines).toEqual(['a', 'b', 'c', 'd', 'e']);
    expect(textLines('')).toEqual([]);
    // An empty line in the text is a line.
    expect(textLines('a\n\nb')).toEqual(['a', '', 'b']);
  });

  it('shows every line exactly once over the pages, and says where each page stands', () => {
    const pages = [0, 1, 2].map((page) => textPage(lines, page, 2));
    expect(pages.map((page) => page.text)).toEqual(['a\nb', 'c\nd', 'e']);
    expect(pages.map((page) => [page.firstLine, page.lastLine, page.totalLines])).toEqual([
      [1, 2, 5],
      [3, 4, 5],
      [5, 5, 5],
    ]);
    expect(pages.every((page) => page.pages === 3)).toBe(true);
    expect(pages.flatMap((page) => textLines(page.text))).toEqual(lines);
  });

  it('gives the nearest page there is for one that does not exist', () => {
    expect(textPage(lines, 99, 2).page).toBe(2);
    expect(textPage(lines, -3, 2).page).toBe(0);
    expect(textPage(lines, Number.NaN, 2).page).toBe(0);
  });

  it('is one page for a text that fits, and for no text at all', () => {
    expect(textPage(lines, 0)).toMatchObject({ pages: 1, firstLine: 1, lastLine: 5, text: 'a\nb\nc\nd\ne' });
    expect(textPage([], 0)).toMatchObject({ pages: 1, firstLine: 0, lastLine: 0, totalLines: 0, text: '' });
  });

  it('keeps an empty line at the end of a page, which a bare line break would lose', () => {
    const page = textPage(['a', '', 'b'], 0, 2);
    expect(textLines(page.text)).toEqual(['a', '']);
    expect(page.lastLine).toBe(2);
  });
});

describe('a stored chart, drawn again', () => {
  it('lifts the pen across a stretch nobody recorded instead of joining the two ends', () => {
    const chart = storedChart(holed());
    expect(chart.gaps).toEqual([{ from: T0 + 3 * MINUTE, to: T0 + 24 * MINUTE }]);
    expect(chart.missingMs).toBe(21 * MINUTE);
    // One extra time inside the hole, and nobody has a value for it.
    const filler = chart.timestamps.indexOf(T0 + 4 * MINUTE);
    expect(filler).toBe(4);
    expect(chart.series[0]?.values[filler]).toBeNull();
    // Either side of it the stored values are untouched.
    expect(chart.series[0]?.values).toEqual([0, 10, 20, 30, null, 40, 50, 60]);
  });

  it('adds no value that was not stored', () => {
    const body = holed();
    const chart = storedChart(body);
    const drawn = chart.series[0]?.values.filter((value) => value !== null);
    const stored = (body.series[0]?.points as [number, number][]).map(([, value]) => value);
    expect(drawn).toEqual(stored);
    // In particular the hole is not a run of zeros.
    expect(chart.series[0]?.values.filter((value) => value === 0)).toHaveLength(1);
  });

  it('leaves one series empty at a time only the other has a sample for, without calling that a gap', () => {
    // Synthetic: two series, the second one missed the middle poll.
    const chart = storedChart({
      type: 'chart',
      series: [
        { points: [[T0, 1], [T0 + MINUTE, 2], [T0 + 2 * MINUTE, 3]], step_s: 60 },
        { points: [[T0, 7], [T0 + 2 * MINUTE, 9]], step_s: 60 },
      ],
      coverage: { since: T0, until: T0 + 3 * MINUTE, points: 5, series: [] },
    });
    expect(chart.gaps).toEqual([]);
    expect(chart.timestamps).toEqual([T0, T0 + MINUTE, T0 + 2 * MINUTE]);
    expect(chart.series[1]?.values).toEqual([7, null, 9]);
  });

  it('reads a real capture: the points as stored, named by the engine’s own coverage', () => {
    const chart = storedChart(REAL_CHART);
    expect(chart.timestamps).toEqual([1795386599000]);
    expect(chart.series.map((series) => series.label)).toEqual(['cpu.pct', 'if.oper · FastEthernet0/1']);
    expect(chart.series.map((series) => series.values)).toEqual([[2], [0]]);
    expect(chart.gaps).toEqual([]);
    expect(gapSentence(chart)).toBeNull();
    expect(chart.unreadable).toBe(0);
    expect(chart.dropped).toBe(0);
  });

  it('names the device as well when the series are of several devices', () => {
    const body = holed();
    body.series.push({ device_id: 'd2', metric: 'cpu.pct', instance: '', points: [[T0, 5]], step_s: 60 });
    body.coverage.series.push({ device_id: 'd2', device: 'dist-sw1', lab: true, metric: 'cpu.pct', instance: '', points: 1 });
    expect(storedChart(body).series.map((series) => series.label)).toEqual(['core-sw1: cpu.pct', 'dist-sw1: cpu.pct']);
  });

  it('draws nothing from points that are not a time and a number, and says how many', () => {
    // Synthetic: a body no monitor would send. The item stores whatever was
    // answered, so nothing about its shape is taken on trust.
    const chart = storedChart({
      type: 'chart',
      series: [
        { points: [[T0, 1], ['<secret:1>', 2], [T0 + MINUTE, null], [T0 + 2 * MINUTE, Number.NaN], 7, [T0 + 3 * MINUTE, 4]] },
        { points: '<hidden>' },
        {},
      ],
      coverage: { since: T0, until: T0 + MINUTE, points: 0, series: [] },
    } as unknown as Extract<EvidenceBody, { type: 'chart' }>);
    expect(chart.series[0]?.values.filter((value) => value !== null)).toEqual([1, 4]);
    expect(chart.dropped).toBe(4);
    expect(chart.unreadable).toBe(2);
    expect(chart.series).toHaveLength(3);
    expect(chart.series[1]?.values.every((value) => value === null)).toBe(true);
  });

  it('has no times at all for a chart with no points, rather than an axis of nothing', () => {
    const chart = storedChart({
      type: 'chart',
      series: [{ device_id: 'd1', metric: 'cpu.pct', instance: '', points: [], step_s: 60 }],
      coverage: { since: T0, until: T0 + MINUTE, points: 0, series: [] },
    });
    expect(chart.timestamps).toEqual([]);
    expect(chart.gaps).toEqual([]);
  });

  it('says in words how much the picture leaves out', () => {
    const one = gapSentence(storedChart(holed()));
    expect(one).toContain('A gap of');
    expect(one).toContain('nothing was recorded then');
    const two = gapSentence({ gaps: [{ from: 0, to: 600_000 }, { from: 900_000, to: 1_500_000 }], missingMs: 1_200_000 });
    expect(two).toMatch(/^2 gaps, /);
  });
});

describe('the coverage of a stored chart', () => {
  it('says what was read for each series, in the order they were asked for', () => {
    const lines = coverageLines(REAL_CHART.coverage);
    expect(lines.map((line) => line.name)).toEqual(['Ops/SW1: cpu.pct', 'Ops/SW1: if.oper · FastEthernet0/1']);
    expect(lines.every((line) => line.lab)).toBe(true);
    expect(lines[0]?.text).toMatch(/^1 point, at /);
    expect(lines[0]?.text).toContain('raw buckets of');
  });

  it('states the longest stretch between two samples', () => {
    const [line] = coverageLines(holed().coverage);
    expect(line?.text).toContain('7 points');
    expect(line?.text).toContain('longest stretch between two of them');
  });

  it('says that a series has no points, which is not the same as not knowing', () => {
    // Synthetic: the shape the engine records for a series without samples.
    const coverage: EvidenceCoverage = {
      since: T0,
      until: T0 + MINUTE,
      points: 0,
      series: [{ device_id: 'd1', device: 'core-sw1', lab: false, metric: 'if.in_bps', instance: 'Gi0/1', points: 0 }],
    };
    const [line] = coverageLines(coverage);
    expect(line?.empty).toBe(true);
    expect(line?.text).toContain('no points');
    expect(line?.text).toContain('nothing was recorded');
    expect(line?.text).not.toMatch(/\b0\b/);
  });
});

// ------------------------------------------------------------------- the list

describe('what a page of the list is a page of', () => {
  it('says everything is shown only when the engine gave no cursor', () => {
    expect(listCoverage(12, false, false, true)).toBe('All 12 items that are kept.');
    expect(listCoverage(3, false, true, true)).toBe('All 3 items that match.');
  });

  it('says older items exist when it did, and never a total it was not given', () => {
    const text = listCoverage(50, true, false, true);
    expect(text).toBe('The newest 50 items; older ones exist and are not shown.');
    expect(text).not.toMatch(/of \d+/);
    expect(listCoverage(50, true, true, true)).toContain('that match');
  });

  it('tells "nothing kept yet" from "nothing matches"', () => {
    expect(listCoverage(0, false, false, true)).toBe('No evidence has been kept yet.');
    expect(listCoverage(0, false, true, true)).toBe('Nothing matches these filters.');
  });

  it('says where an older page stands', () => {
    expect(listCoverage(50, true, false, false)).toContain('older ones exist beyond it');
    expect(listCoverage(7, false, false, false)).toContain('the oldest there are');
  });
});

// ---------------------------------------------------------------- provenance

describe('where an item came from', () => {
  it('names the session the engine read', () => {
    expect(sourceSentence(TERMINAL.source)).toBe(
      'Read by the engine from the terminal session “Ops/SW1” (lab:Ops/SW1).',
    );
    expect(isOperatorSupplied(TERMINAL.source)).toBe(false);
  });

  it('says plainly when a person supplied it and the engine read nothing', () => {
    expect(isOperatorSupplied(OPERATOR.source)).toBe(true);
    const sentence = sourceSentence(OPERATOR.source);
    expect(sentence).toContain('Supplied by a person');
    expect(sentence).not.toContain('Read by the engine');
  });

  it('still says something true about a source it has never heard of', () => {
    expect(sourceSentence({ from: 'flows.query', lab: false })).toBe('Read by the engine with flows.query.');
  });

  it('knows the five kinds and nothing else', () => {
    for (const kind of ['terminal', 'log', 'text', 'topology', 'chart']) expect(isEvidenceKind(kind)).toBe(true);
    expect(isEvidenceKind('screenshot')).toBe(false);
    expect(isEvidenceKind(null)).toBe(false);
  });
});

// --------------------------------------------------------------- the timeline

describe('evidence on an incident’s timeline', () => {
  it('opens the item it points at', () => {
    expect(attachedEvidenceId(ATTACHED)).toBe('wqqeevuttw2g3hbj');
    expect(isTombstone(ATTACHED)).toBe(false);
    expect(entryLink(ATTACHED, [])).toBe('/ops/evidence?item=wqqeevuttw2g3hbj');
    expect(ENTRY_WORD.evidence).toBe('Evidence');
  });

  it('knows a tombstone for what it is: evidence that was here, and nowhere to go', () => {
    expect(isTombstone(TOMBSTONE)).toBe(true);
    expect(attachedEvidenceId(TOMBSTONE)).toBeNull();
    expect(entryLink(TOMBSTONE, ['d1'])).toBeNull();
    // What it was is still on the record.
    expect(TOMBSTONE.source.evidence).toContain('sha256 8f11468dcbb886c2c4948586af262b4abf786e68364ff3be22f67446bfe4618a');
  });

  it('does not mistake any other entry without a reference for a tombstone', () => {
    const note: TimelineEntry = { ...TOMBSTONE, id: 'note:1', kind: 'note' };
    expect(isTombstone(note)).toBe(false);
    expect(attachedEvidenceId({ ...ATTACHED, kind: 'log' })).toBeNull();
  });

  it('can be told from a deleted record in the audit trail', () => {
    expect(RESOLVABLE_KINDS.has('evidence')).toBe(true);
    expect(refLink({ type: 'evidence', id: 'a b' })).toBe('/ops/evidence?item=a%20b');
  });
});

describe('before an item is deleted', () => {
  const incidents = [
    { id: 'i1', title: 'SW1 uplink flapping' },
    { id: 'i2', title: 'Core reload' },
  ];

  it('names the incidents that will keep a tombstone', () => {
    const text = deleteWarning({ ...TERMINAL, incident_ids: ['i1', 'i2'] }, incidents);
    expect(text).toContain('“SW1 uplink flapping” and “Core reload”');
    expect(text).toContain('their timeline');
    expect(text).toContain('tombstone');
    expect(text).toContain('cannot be undone');
  });

  it('names an incident it has no title for by its id rather than leaving it out', () => {
    const text = deleteWarning({ ...TERMINAL, incident_ids: ['i1', 'zz9'] }, incidents);
    expect(text).toContain('“SW1 uplink flapping” and “incident zz9”');
  });

  it('says so when no incident is affected', () => {
    const text = deleteWarning(TERMINAL, incidents);
    expect(text).toContain('not attached to any incident');
    expect(text).not.toContain('tombstone');
  });
});

// ------------------------------------------------------------------- a drawing

describe('the address a stored drawing is shown from', () => {
  it('is a data address of an SVG image, with nothing of the drawing in it as text', () => {
    // Synthetic and hostile: a script, an onload and an address to fetch.
    const svg =
      '<svg xmlns="http://www.w3.org/2000/svg" onload="window.__pwned = 1"><script>window.__pwned = 2</script><image href="http://evil.example/x.png"/></svg>';
    const src = svgImageSrc(svg);
    expect(src.startsWith('data:image/svg+xml;base64,')).toBe(true);
    expect(src.startsWith('blob:')).toBe(false);
    expect(src).not.toContain('<');
    expect(src).not.toContain('script');
    expect(src).not.toContain('evil.example');
    expect(atob(src.slice('data:image/svg+xml;base64,'.length))).toBe(svg);
  });
});

// ------------------------------------------------------------------- refusals

describe('an answer that is a no', () => {
  it('keeps the engine’s own reason for a capture it could not make', () => {
    // The message is the engine’s, from the backend’s recorded run.
    const error = new RpcError(
      'unavailable',
      'evidence cannot be captured from a session log: `terminal.logs.list` is not part of this build',
    );
    expect(failure(error)).toEqual({
      headline: 'The engine could not read it',
      message: 'evidence cannot be captured from a session log: `terminal.logs.list` is not part of this build',
    });
  });

  it('has a headline for each kind of refusal and never loses the sentence', () => {
    expect(failure(new RpcError('not_found', 'terminal session `nope` was not found')).headline).toBe('It is not there');
    expect(failure(new RpcError('invalid', 'a title has at most 200 characters')).headline).toBe('It was refused');
    expect(failure(new Error('socket closed'))).toEqual({ headline: 'It did not work', message: 'socket closed' });
  });
});

// ---------------------------------------------------------------------- files

describe('a file made from an item', () => {
  it('is named after the item, with its id, and nothing a file system minds', () => {
    expect(evidenceFilename(TERMINAL, 'txt')).toBe('ops-sw1-last-25-lines-lks55ph2ugbxlatu.txt');
    expect(evidenceFilename({ id: 'a1', title: '../../etc/passwd' }, 'svg')).toBe('etc-passwd-a1.svg');
    expect(evidenceFilename({ id: 'a1', title: '···' }, 'png')).toBe('evidence-a1.png');
  });
});

// --------------------------------------------------------------- the handover

/** The parts of a handover this section does not depend on, empty. */
function handover(evidence: Handover['evidence'], unavailable: Handover['unavailable'] = []): Handover {
  return {
    period: { since: 1_000, until: 9_000 },
    alerts: null,
    incidents: { opened: [], resolved: [], open: [] },
    changes: null,
    config_changes: null,
    devices: {
      added: [],
      removed: [],
      went_down: [],
      unreachable_now: [],
      without_reachability_samples: 0,
      not_measured: 0,
    },
    backups: null,
    jobs_failed: [],
    ...(evidence === undefined ? {} : { evidence }),
    notes: '',
    unavailable,
    generated_at: 9_000,
  };
}

describe('the evidence section of a handover', () => {
  const section = (value: Handover) => handoverSections(value).find((entry) => entry.id === 'evidence');

  it('lists what was captured, each item linked to the item itself and never carrying its content', () => {
    const found = section(handover({ captured: [TERMINAL, OPERATOR] }));
    expect(found?.summary).toBe('2 kept');
    expect(found?.body.kind).toBe('items');
    const items = found?.body.kind === 'items' ? found.body.items : [];
    expect(items.map((item) => item.to)).toEqual([
      '/ops/evidence?item=lks55ph2ugbxlatu',
      '/ops/evidence?item=xjgc4vs5b45kt32b',
    ]);
    expect(items[0]?.lab).toBe(true);
    expect(items[0]?.detail).toContain('terminal.snapshot');
    expect(items[0]?.detail).toContain('sha256 d7587f1d4594');
    // Text a person handed in says so here too.
    expect(items[1]?.detail).toContain('supplied by a person');
  });

  it('says nothing was kept when the engine looked and found nothing', () => {
    const found = section(handover({ captured: [] }));
    expect(found?.body).toEqual({ kind: 'nothing', sentence: 'Nothing was kept as evidence in this period.' });
  });

  it('says why when the engine could not look, instead of "nothing was kept"', () => {
    const found = section(
      handover(null, [{ section: 'evidence', reason: 'failed', message: 'the table could not be read' }]),
    );
    expect(found?.body).toEqual({ kind: 'unavailable', reason: 'failed', message: 'the table could not be read' });
  });

  it('has no section at all for a handover saved before evidence existed', () => {
    expect(section(handover(undefined))).toBeUndefined();
  });
});
