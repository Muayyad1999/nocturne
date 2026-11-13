/**
 * The contrast contract of the design tokens, checked in both themes.
 * WCAG AA asks for 4.5:1 for text; these are the pairings the components use.
 */
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(new URL('./tokens.css', import.meta.url), 'utf8');

type Rgb = [number, number, number];
interface Color {
  rgb: Rgb;
  alpha: number;
}

/** The declarations of the first rule whose selector is `selector`. */
function declarations(selector: string): Map<string, string> {
  const start = css.indexOf(`\n${selector} {`);
  if (start < 0) throw new Error(`no rule for ${selector}`);
  const body = css.slice(css.indexOf('{', start) + 1, css.indexOf('\n}', start));
  const found = new Map<string, string>();
  for (const [, name, value] of body.matchAll(/(--[\w-]+):\s*([^;]+);/g)) {
    if (name && value) found.set(name, value.trim());
  }
  return found;
}

const night = declarations(':root');
const dayOverrides = declarations("[data-theme='light']");
const day = new Map([...night, ...dayOverrides]);

function parse(value: string): Color {
  const hex = /^#([0-9a-f]{6})$/i.exec(value);
  if (hex?.[1]) {
    const number = Number.parseInt(hex[1], 16);
    return { rgb: [number >> 16, (number >> 8) & 0xff, number & 0xff], alpha: 1 };
  }
  const rgba = /^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/.exec(value);
  if (rgba) return { rgb: [Number(rgba[1]), Number(rgba[2]), Number(rgba[3])], alpha: Number(rgba[4]) };
  throw new Error(`not a colour: ${value}`);
}

function token(theme: Map<string, string>, name: string): Color {
  let value = theme.get(name);
  // Follow `var(--other)` references.
  for (let reference = /^var\((--[\w-]+)\)$/.exec(value ?? ''); reference; reference = /^var\((--[\w-]+)\)$/.exec(value ?? '')) {
    value = theme.get(reference[1] ?? '');
  }
  if (!value) throw new Error(`token ${name} is not defined`);
  return parse(value);
}

/** A translucent colour as it appears over an opaque background. */
function over(foreground: Color, background: Color): Color {
  const blend = (index: 0 | 1 | 2) =>
    foreground.rgb[index] * foreground.alpha + background.rgb[index] * (1 - foreground.alpha);
  return { rgb: [blend(0), blend(1), blend(2)], alpha: 1 };
}

function luminance({ rgb }: Color): number {
  const [r, g, b] = rgb.map((channel) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  }) as Rgb;
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: Color, b: Color): number {
  const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (high + 0.05) / (low + 0.05);
}

const AA = 4.5;
const SURFACES = ['--bg-0', '--bg-1', '--bg-2', '--bg-3'];
const STATUSES = ['--ok', '--warn', '--crit', '--info', '--idle', '--maint'];
const THEMES: [string, Map<string, string>][] = [
  ['Night', night],
  ['Day', day],
];

describe.each(THEMES)('%s theme', (_name, theme) => {
  const ratio = (text: string, background: string) => contrast(token(theme, text), token(theme, background));
  const ratioOnSoft = (text: string, soft: string, surface: string) =>
    contrast(token(theme, text), over(token(theme, soft), token(theme, surface)));

  it.each(['--text-1', '--text-2'])('%s passes AA on every surface, including the selected one', (text) => {
    for (const surface of [...SURFACES, '--bg-4']) {
      expect(ratio(text, surface), `${text} on ${surface}`).toBeGreaterThanOrEqual(AA);
    }
  });

  it('muted text passes AA on resting and hover surfaces', () => {
    for (const surface of SURFACES) {
      expect(ratio('--text-3', surface), `--text-3 on ${surface}`).toBeGreaterThanOrEqual(AA);
    }
  });

  it.each(['--accent', '--accent-strong', '--ai', ...STATUSES])('%s passes AA as text', (color) => {
    for (const surface of SURFACES) {
      expect(ratio(color, surface), `${color} on ${surface}`).toBeGreaterThanOrEqual(AA);
    }
  });

  it.each(['--ai', ...STATUSES])('%s passes AA on its own soft fill', (color) => {
    for (const surface of SURFACES) {
      expect(ratioOnSoft(color, `${color}-soft`, surface), `${color} on ${color}-soft over ${surface}`).toBeGreaterThanOrEqual(AA);
    }
  });

  it('accent text on the accent fill uses --accent-strong, which passes AA', () => {
    for (const surface of SURFACES) {
      expect(ratioOnSoft('--accent-strong', '--accent-soft', surface), `over ${surface}`).toBeGreaterThanOrEqual(AA);
    }
  });

  it('body text stays readable on a selected (accent-soft) row', () => {
    for (const text of ['--text-1', '--text-2']) {
      for (const surface of SURFACES) {
        expect(ratioOnSoft(text, '--accent-soft', surface), `${text} over ${surface}`).toBeGreaterThanOrEqual(AA);
      }
    }
  });

  it.each(['--accent', '--accent-strong', '--ai', '--crit'])('text on a solid %s button passes AA', (fill) => {
    expect(ratio('--text-on-solid', fill)).toBeGreaterThanOrEqual(AA);
  });

  it('every soft fill is its own colour at a low alpha', () => {
    for (const color of ['--accent', '--ai', ...STATUSES]) {
      const solid = token(theme, color);
      const soft = token(theme, `${color}-soft`);
      expect(soft.rgb, `${color}-soft`).toEqual(solid.rgb);
      expect(soft.alpha).toBeGreaterThanOrEqual(0.1);
      expect(soft.alpha).toBeLessThanOrEqual(0.16);
    }
  });

  it('chart marks are visible on the chart surfaces (3:1)', () => {
    for (const surface of ['--bg-0', '--bg-1', '--bg-2']) {
      expect(ratio('--chart-muted', surface), `--chart-muted on ${surface}`).toBeGreaterThanOrEqual(3);
    }
  });
});

describe('theme structure', () => {
  it('Day overrides every colour of Night and nothing else', () => {
    const colours = [...night].filter(([, value]) => /^(#|rgba\()/.test(value)).map(([name]) => name);
    const shadows = ['--shadow-float', '--shadow-modal'];
    expect([...dayOverrides.keys()].sort()).toEqual([...colours, ...shadows].sort());
  });

  it('defines the eight chart series in both themes', () => {
    for (const theme of [night, day]) {
      const series = Array.from({ length: 8 }, (_, index) => theme.get(`--series-${index + 1}`));
      expect(new Set(series).size).toBe(8);
      expect(series.every((value) => /^#[0-9a-f]{6}$/.test(value ?? ''))).toBe(true);
    }
  });

  it('keeps the spec values that define the brand', () => {
    expect(night.get('--bg-0')).toBe('#090d18');
    expect(night.get('--accent')).toBe('#7c9cff');
    expect(night.get('--ai')).toBe('#c2a5ff');
    expect(day.get('--bg-0')).toBe('#f6f7fb');
    expect(day.get('--bg-1')).toBe('#ffffff');
    expect(day.get('--text-1')).toBe('#1a2233');
    expect(day.get('--accent')).toBe('#3d5afe');
  });
});
