/**
 * The environment's locale, as something `Intl` accepts.
 *
 * Nocturne writes its own numbers and times (`format.ts`, `time.ts`), so its
 * own code needs no locale. Libraries do: they hand `navigator.language` to
 * an `Intl` constructor, which throws a RangeError for anything that is not a
 * language tag. Chromium always reports one. WebKitGTK reports the process
 * locale, so a Linux desktop started with `LANG` unset, `C`, `C.UTF-8` or
 * `POSIX` says `"C"` — and the first screen to load such a library is gone.
 *
 * Anything that builds an `Intl` formatter from the environment's locale
 * takes it from `environmentLocale()`, never from `navigator` directly. Code
 * that is not ours reads `navigator` itself; `repairNavigatorLocale()` makes
 * what it finds there safe, and has to run before that code is evaluated.
 */

/** What the C locale formats like, and the language the interface is written in. */
export const FALLBACK_LOCALE = 'en-US';

/** What `navigator` says about language. Loose, because a WebView's answer is not to be trusted. */
export interface LocaleSource {
  readonly language?: unknown;
  readonly languages?: unknown;
}

/** Whether an `Intl` constructor takes this tag rather than throwing. */
export function isLocaleTag(tag: unknown): tag is string {
  if (typeof tag !== 'string') return false;
  try {
    // The same check every `Intl` constructor makes on its `locales` argument.
    return Intl.getCanonicalLocales(tag).length === 1;
  } catch {
    return false;
  }
}

function offered(source: LocaleSource | undefined): unknown[] {
  if (!source) return [];
  const languages: unknown = source.languages;
  return [source.language, ...(Array.isArray(languages) ? (languages as unknown[]) : [])];
}

/**
 * The locales the environment asks for, most preferred first, without the
 * ones `Intl` would refuse. Never empty: with nothing usable it is English,
 * rather than a second language the person did not ask for either.
 */
export function environmentLocales(source: LocaleSource | undefined = globalThis.navigator): string[] {
  const usable = [...new Set(offered(source).filter(isLocaleTag))];
  return usable.length > 0 ? usable : [FALLBACK_LOCALE];
}

/** The locale to give an `Intl` constructor when the environment's is wanted. */
export function environmentLocale(source: LocaleSource | undefined = globalThis.navigator): string {
  return environmentLocales(source)[0] ?? FALLBACK_LOCALE;
}

/**
 * Makes `navigator.language` and `navigator.languages` safe to hand to
 * `Intl`, for libraries that do exactly that. A navigator that already
 * reports language tags is left as the browser made it.
 */
export function repairNavigatorLocale(target: LocaleSource | undefined = globalThis.navigator): void {
  if (!target || offered(target).every(isLocaleTag)) return;
  const languages = environmentLocales(target);
  try {
    // Own properties: in a browser both are accessors on `Navigator.prototype`,
    // and the instance is the only place a page may overrule them.
    Object.defineProperty(target, 'language', { value: languages[0], configurable: true, enumerable: true });
    Object.defineProperty(target, 'languages', {
      value: Object.freeze(languages),
      configurable: true,
      enumerable: true,
    });
  } catch {
    // A navigator that cannot be redefined. `environmentLocale()` is still
    // right for our own code; a library that reads `navigator` is on its own.
  }
}
