// @vitest-environment happy-dom
/**
 * The chart has to load on a desktop whose locale is not a language.
 *
 * WebKitGTK reports the process locale as `navigator.language`, so a Linux
 * desktop started with `LANG` unset, `C`, `C.UTF-8` or `POSIX` says `"C"`.
 * uPlot builds `new Intl.NumberFormat(navigator.language)` while its module
 * is being evaluated, and `"C"` is a RangeError there — which took the whole
 * Monitoring screen with it, because the chart is what that screen's code
 * imports. Chromium never reports such a tag, so only this file sees it.
 *
 * The chart is imported inside the test, after the locale has been set: the
 * failure is in loading the module, not in anything it does afterwards.
 */

import { describe, expect, it } from 'vitest';

describe('where the WebView reports the C locale as its language', () => {
  it('the chart still loads, and what uPlot reads is a tag Intl accepts', async () => {
    Object.defineProperty(navigator, 'language', { value: 'C', configurable: true });
    Object.defineProperty(navigator, 'languages', { value: ['C'], configurable: true });
    expect(() => new Intl.NumberFormat(navigator.language), 'the premise: this tag is refused').toThrow(RangeError);

    const chart = await import('./TimeSeriesChart');

    expect(chart.TimeSeriesChart).toBeTypeOf('function');
    expect(new Intl.NumberFormat(navigator.language).format(1234.5)).toBe('1,234.5');
    expect(() => new Intl.NumberFormat([...navigator.languages])).not.toThrow();
  });
});
