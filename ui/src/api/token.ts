/**
 * The access token for server mode. The server prints a link ending in
 * `#token=…`; the token is taken from the fragment on first load, kept for the
 * tab's lifetime in sessionStorage, and removed from the address bar so it is
 * neither bookmarked nor mistaken for a route by the hash router.
 */

const STORAGE_KEY = 'nocturne.token';

export interface TokenEnvironment {
  location: Pick<Location, 'hash' | 'pathname' | 'search'>;
  history: Pick<History, 'replaceState'>;
  storage: Pick<Storage, 'getItem' | 'setItem'>;
}

function browserEnvironment(): TokenEnvironment {
  return { location: window.location, history: window.history, storage: window.sessionStorage };
}

/**
 * Returns the access token, capturing it from the URL fragment if present.
 * Call once at start-up, before the router reads the fragment.
 */
export function captureToken(environment: TokenEnvironment = browserEnvironment()): string | null {
  const { location, history, storage } = environment;
  const fragment = location.hash.replace(/^#/, '');

  if (fragment.startsWith('token=')) {
    const token = new URLSearchParams(fragment).get('token');
    history.replaceState(null, '', `${location.pathname}${location.search}`);
    if (token) {
      try {
        storage.setItem(STORAGE_KEY, token);
      } catch {
        // Storage is unavailable: the token still works until the page reloads.
      }
      return token;
    }
  }

  try {
    return storage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}
