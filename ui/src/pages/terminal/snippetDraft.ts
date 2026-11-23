/**
 * What the snippet form holds while a snippet is being written or changed.
 *
 * `terminal.snippets.save` replaces the whole record, so the draft carries
 * every field of it, not only the ones the form used to show. A draft that
 * held just the name and the text erased a snippet's tags on every edit, and
 * rewrote the device type it was for to that of whichever session happened to
 * be in front.
 */

import type { Snippet, SnippetSaveParams } from '@/api/types/terminal';

export interface SnippetDraft {
  /** The snippet being changed, or null for a new one. */
  id: string | null;
  name: string;
  text: string;
  /** The device type it is for; null means any. */
  profile: string | null;
  tags: string[];
  /** The type it had when the form opened, so the choice to go back to it stays on offer. */
  savedProfile: string | null;
}

/** The value the device-type choice uses for "any": a profile id is never empty. */
export const ANY_PROFILE = '';

/**
 * A new snippet. It starts out for the device type of the session in front —
 * a command typed at a Cisco prompt is rarely meant for a Juniper one — and
 * the form says so, where the person can change it.
 */
export function newDraft(sessionProfile: string | null): SnippetDraft {
  return { id: null, name: '', text: '', profile: sessionProfile, tags: [], savedProfile: null };
}

/** An existing snippet, exactly as it is saved. */
export function draftFor(snippet: Snippet): SnippetDraft {
  const profile = snippet.profile ?? null;
  return {
    id: snippet.id,
    name: snippet.name,
    text: snippet.text,
    profile,
    tags: [...snippet.tags],
    savedProfile: profile,
  };
}

/** The draft with the device-type choice set to one of `profileChoices`' values. */
export function withProfileChoice(draft: SnippetDraft, value: string): SnippetDraft {
  return { ...draft, profile: value === ANY_PROFILE ? null : value };
}

export interface ProfileChoice {
  value: string;
  label: string;
}

/**
 * What the snippet can be for: any device, the type it already has, and the
 * type of the session in front. Profile ids are shown as they are — they are
 * what the device list and the snippet rows show too.
 */
export function profileChoices(draft: SnippetDraft, sessionProfile: string | null): ProfileChoice[] {
  const choices: ProfileChoice[] = [{ value: ANY_PROFILE, label: 'Any device' }];
  for (const profile of [draft.savedProfile, draft.profile, sessionProfile]) {
    if (!profile || choices.some((choice) => choice.value === profile)) continue;
    choices.push({
      value: profile,
      label: profile === sessionProfile ? `${profile} only (this session)` : `${profile} only`,
    });
  }
  return choices;
}

/** What `terminal.snippets.save` is sent: the whole record. */
export function saveParams(draft: SnippetDraft): SnippetSaveParams {
  return {
    id: draft.id,
    name: draft.name.trim(),
    text: draft.text,
    profile: draft.profile,
    tags: draft.tags,
  };
}
