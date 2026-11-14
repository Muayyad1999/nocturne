/**
 * The live tail, and paging back through what is stored.
 *
 * The tail is a **server-side** filtered stream: `logs.follow` registers this
 * screen's filter with the engine and the batches come back on `logs.tail`
 * with a `follow_id`. The screen never receives the whole firehose to sieve in
 * the browser, which at twenty thousand lines a minute is the difference
 * between a tail and a frozen tab. A follow expires after two minutes unless
 * it is renewed, so a tab that is closed or crashes stops costing the engine a
 * filter pass per line by itself.
 *
 * `logs.appended` is the unfiltered tail that every screen gets. While the live
 * tail is off it is used for one thing only: to say that lines *have* arrived,
 * without claiming how many of them match the search — because that event
 * cannot know.
 */

import { useEffect, useEffectEvent, useRef, useState } from 'react';
import { rpc } from '@/api/client';
import { useEvent } from '@/api/hooks';
import type { Follow, LogFilter, LogPage, LogsAppended, LogsTail } from '@/api/types/logs';
import { PAGE, useLogPage } from './data';
import { EMPTY_STREAM, adopt, append, entryCount, fromPage, prepend, type Stream } from './stream';

/** How often a follow is renewed. Its time to live is two minutes. */
const RENEW_MS = 45_000;

export interface LogStream {
  stream: Stream;
  /** The first page is still on its way. */
  loading: boolean;
  error: unknown;
  retry: () => void;
  /** Older lines than the view holds are stored. */
  hasOlder: boolean;
  loadingOlder: boolean;
  loadOlder: () => void;
  /** Lines have arrived since this view was read, and the tail is off. */
  behind: boolean;
  /** Read the newest page again (what the "behind" notice offers). */
  refresh: () => void;
  /** The engine is streaming this filter to us right now. */
  following: boolean;
  /** Why it is not, when the tail is on and the follow could not be opened. */
  followError: string | null;
}

/**
 * One filter's worth of lines: the newest page, older pages on demand, and the
 * tail appended when `tail` is on.
 *
 * `filterKey` is the serialised filter: changing the search throws the view
 * away rather than mixing two searches in one list.
 */
export function useLogStream(filter: LogFilter, tail: boolean, enabled: boolean): LogStream {
  const page = useLogPage(filter, enabled);
  const [stream, setStream] = useState<Stream>(EMPTY_STREAM);
  const [hasMore, setHasMore] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [behind, setBehind] = useState(false);
  const [following, setFollowing] = useState(false);
  const [followError, setFollowError] = useState<string | null>(null);

  // The page query is keyed by the filter, so new data means a new search or a
  // refetch of the same one; `shown` says which, because they are not the
  // same thing for a view the tail is adding to.
  const data = page.data;
  const filterKey = JSON.stringify(filter);
  const shown = useRef(filterKey);
  useEffect(() => {
    if (data === undefined) return;
    // A page of the *same* search keeps whatever the tail delivered while it
    // was being read — that line belongs on the end of it, not in the gap
    // between the two. A page of a *different* search keeps nothing: the
    // lines in the view were chosen by the filter that has just changed.
    const same = shown.current === filterKey;
    shown.current = filterKey;
    setStream((current) => (same ? adopt(current, data.entries) : fromPage(data.entries)));
    setHasMore(data.has_more);
    setBehind(false);
  }, [data, filterKey]);

  // The filter as it is now, for calls made from an interval rather than from
  // a render; the effect itself restarts whenever `filterKey` changes.
  const filterRef = useRef(filter);
  filterRef.current = filter;
  /** The follow the engine is streaming to us, for matching its events. */
  const followRef = useRef<string | null>(null);

  // ---------------------------------------------------------------- the tail

  useEffect(() => {
    if (!tail || !enabled) {
      setFollowing(false);
      setFollowError(null);
      return;
    }
    let live = true;
    let id: string | null = null;

    const open = async () => {
      try {
        const follow = await rpc<Follow>('logs.follow', {
          ...filterRef.current,
          ...(id === null ? {} : { follow_id: id }),
        });
        if (!live) {
          // Unmounted while the call was in flight: close what was opened
          // rather than leaving the engine filtering for nobody.
          void rpc('logs.unfollow', { id: follow.follow_id }).catch(() => undefined);
          return;
        }
        id = follow.follow_id;
        followRef.current = follow.follow_id;
        setFollowing(true);
        setFollowError(null);
      } catch (error) {
        if (!live) return;
        setFollowing(false);
        setFollowError(error instanceof Error ? error.message : String(error));
      }
    };

    void open();
    const timer = setInterval(() => void open(), RENEW_MS);
    return () => {
      live = false;
      clearInterval(timer);
      setFollowing(false);
      followRef.current = null;
      if (id !== null) void rpc('logs.unfollow', { id }).catch(() => undefined);
    };
  }, [filterKey, tail, enabled]);

  useEvent<LogsTail>('logs.tail', (payload) => {
    if (!tail || payload.follow_id !== followRef.current) return;
    setStream((current) => append(current, payload));
  });

  // The unfiltered tail. It cannot say whether these lines match the search,
  // so it is only allowed to say that something arrived.
  useEvent<LogsAppended>('logs.appended', () => {
    if (!tail && enabled) setBehind(true);
  });

  // ------------------------------------------------------------ older pages

  const loadOlder = useEffectEvent(() => {
    const before = stream.oldest;
    if (before === null || loadingOlder || !hasMore) return;
    setLoadingOlder(true);
    void rpc<LogPage>('logs.query', { ...filter, before_id: before, limit: PAGE })
      .then((older) => {
        setStream((current) => prepend(current, older.entries));
        setHasMore(older.has_more);
      })
      .catch(() => {
        // A failed page back is not worth a toast on a screen that is
        // already showing lines: the button stays, and pressing it retries.
      })
      .finally(() => setLoadingOlder(false));
  });

  const refresh = useEffectEvent(() => {
    setBehind(false);
    void page.refetch();
  });

  return {
    stream,
    loading: page.isPending && enabled,
    error: page.error,
    retry: () => void page.refetch(),
    hasOlder: hasMore && entryCount(stream) > 0,
    loadingOlder,
    loadOlder: () => loadOlder(),
    behind,
    refresh: () => refresh(),
    following,
    followError,
  };
}
