import { describe, expect, it } from 'vitest';
import { FALLBACK_LOCALE, environmentLocale, environmentLocales, isLocaleTag, repairNavigatorLocale } from './locale';

/** What WebKitGTK reports with `LANG` unset, `C`, `C.UTF-8` or `POSIX`. */
const C_LOCALE = { language: 'C', languages: ['C'] };

/** A navigator shaped like a browser's: both values are accessors on the prototype. */
function browserNavigator(language: unknown, languages: unknown) {
  class FakeNavigator {
    get language() {
      return language;
    }
    get languages() {
      return languages;
    }
  }
  return new FakeNavigator();
}

describe('telling a language tag from a process locale', () => {
  it('refuses exactly what an Intl constructor refuses', () => {
    for (const tag of ['C', '', 'en_US.UTF-8', 'C.UTF-8', 'en-', 'x', undefined, null, 42, ['en']]) {
      expect(isLocaleTag(tag), String(tag)).toBe(false);
      if (typeof tag === 'string') expect(() => new Intl.NumberFormat(tag), tag).toThrow(RangeError);
    }
    for (const tag of ['en', 'en-US', 'de-CH', 'zh-Hant-TW', 'sr-Latn', 'en-US-u-nu-latn']) {
      expect(isLocaleTag(tag), tag).toBe(true);
      expect(() => new Intl.NumberFormat(tag), tag).not.toThrow();
    }
  });
});

describe('the locale to build a formatter with', () => {
  it('is English where the environment reports the C locale', () => {
    expect(environmentLocale(C_LOCALE)).toBe(FALLBACK_LOCALE);
    expect(new Intl.NumberFormat(environmentLocale(C_LOCALE)).format(1234.5)).toBe('1,234.5');
  });

  it('is what the environment asked for when that is a language', () => {
    expect(environmentLocale({ language: 'de-CH', languages: ['de-CH', 'de', 'en'] })).toBe('de-CH');
    expect(environmentLocales({ language: 'de-CH', languages: ['de-CH', 'de', 'en'] })).toEqual(['de-CH', 'de', 'en']);
  });

  it('is the next preference, not English, when only the first choice is unusable', () => {
    expect(environmentLocale({ language: 'C', languages: ['C', 'fr-FR', 'en'] })).toBe('fr-FR');
  });

  it('is English where there is no navigator, or one that reports nothing', () => {
    expect(environmentLocale(undefined)).toBe(FALLBACK_LOCALE);
    expect(environmentLocale({})).toBe(FALLBACK_LOCALE);
    expect(environmentLocales({ language: undefined, languages: 'C' })).toEqual([FALLBACK_LOCALE]);
  });
});

describe('repairing navigator for code that reads it directly', () => {
  it('replaces the C locale, so that Intl takes what a library hands it', () => {
    const navigator = browserNavigator('C', Object.freeze(['C']));
    expect(() => new Intl.NumberFormat(navigator.language as string)).toThrow(RangeError);

    repairNavigatorLocale(navigator);

    expect(navigator.language).toBe('en-US');
    expect(navigator.languages).toEqual(['en-US']);
    expect(new Intl.NumberFormat(navigator.language as string).format(1234.5)).toBe('1,234.5');
    expect(() => new Intl.DateTimeFormat(navigator.languages as string[])).not.toThrow();
  });

  it('keeps the usable preferences and drops only the ones Intl refuses', () => {
    const navigator = browserNavigator('C', ['C', 'fr-FR', 'en']);
    repairNavigatorLocale(navigator);
    expect(navigator.language).toBe('fr-FR');
    expect(navigator.languages).toEqual(['fr-FR', 'en']);
  });

  it('repairs a preference list with one bad entry even when the first choice is fine', () => {
    const navigator = browserNavigator('en-GB', ['en-GB', 'C']);
    repairNavigatorLocale(navigator);
    expect(navigator.language).toBe('en-GB');
    expect(navigator.languages).toEqual(['en-GB']);
  });

  it('leaves a navigator that reports language tags exactly as the browser made it', () => {
    const languages = Object.freeze(['de-CH', 'de']);
    const navigator = browserNavigator('de-CH', languages);
    repairNavigatorLocale(navigator);
    expect(Object.getOwnPropertyNames(navigator)).toEqual([]);
    expect(navigator.languages).toBe(languages);
  });

  it('does not throw where navigator is missing or sealed', () => {
    expect(() => repairNavigatorLocale(undefined)).not.toThrow();
    const sealed = Object.freeze({ language: 'C', languages: ['C'] });
    expect(() => repairNavigatorLocale(sealed)).not.toThrow();
    // What our own code asks for is right even then.
    expect(environmentLocale(sealed)).toBe('en-US');
  });
});
