import { describe, expect, it } from 'vitest';
import type { Snippet } from '@/api/types/terminal';
import { ANY_PROFILE, draftFor, newDraft, profileChoices, saveParams, withProfileChoice } from './snippetDraft';

const snippet = (over: Partial<Snippet> = {}): Snippet => ({
  id: 'snip-1',
  name: 'Clock',
  text: 'show clock',
  profile: null,
  tags: ['daily', 'ops'],
  created_at: 1,
  updated_at: 2,
  variables: [],
  ...over,
});

describe('editing a snippet', () => {
  it('sends back the tags and the device type it was saved with', () => {
    // The backend replaces the whole record: what is not sent is erased.
    const draft = { ...draftFor(snippet()), name: 'Clock, twice a day' };

    expect(saveParams(draft)).toEqual({
      id: 'snip-1',
      name: 'Clock, twice a day',
      text: 'show clock',
      profile: null,
      tags: ['daily', 'ops'],
    });
  });

  it('does not make a snippet for any device belong to the device it was edited from', () => {
    // Edited from a Cisco session, it must still be there for a Juniper one.
    const draft = draftFor(snippet({ profile: null }));

    expect(saveParams(draft).profile).toBeNull();
  });

  it('keeps a snippet with the device type it has, whatever session is in front', () => {
    const draft = draftFor(snippet({ profile: 'juniper_junos' }));

    expect(saveParams(draft).profile).toBe('juniper_junos');
  });

  it('changes the device type and the tags only when the person does', () => {
    let draft = draftFor(snippet());
    draft = withProfileChoice(draft, 'cisco_ios');
    draft = { ...draft, tags: [...draft.tags, 'ntp'] };

    expect(saveParams(draft)).toMatchObject({ profile: 'cisco_ios', tags: ['daily', 'ops', 'ntp'] });
    expect(saveParams(withProfileChoice(draft, ANY_PROFILE)).profile).toBeNull();
  });
});

describe('a new snippet', () => {
  it('starts out for the device type in front of the person, and can be made general', () => {
    expect(saveParams({ ...newDraft('cisco_ios'), name: ' Version ', text: 'show version' })).toEqual({
      id: null,
      name: 'Version',
      text: 'show version',
      profile: 'cisco_ios',
      tags: [],
    });
  });

  it('is for any device when no session says what the device is', () => {
    expect(newDraft(null).profile).toBeNull();
  });
});

describe('profileChoices', () => {
  it('offers any device, this device type, and the type the snippet already has', () => {
    expect(profileChoices(draftFor(snippet({ profile: 'juniper_junos' })), 'cisco_ios')).toEqual([
      { value: ANY_PROFILE, label: 'Any device' },
      { value: 'juniper_junos', label: 'juniper_junos only' },
      { value: 'cisco_ios', label: 'cisco_ios only (this session)' },
    ]);
  });

  it('does not list a type twice, and offers nothing it cannot name', () => {
    expect(profileChoices(draftFor(snippet({ profile: 'cisco_ios' })), 'cisco_ios')).toEqual([
      { value: ANY_PROFILE, label: 'Any device' },
      { value: 'cisco_ios', label: 'cisco_ios only (this session)' },
    ]);
    expect(profileChoices(draftFor(snippet()), null)).toEqual([{ value: ANY_PROFILE, label: 'Any device' }]);
  });

  it('still shows a type that was chosen and then left for another', () => {
    // The list must not lose the option the form's value came from.
    const draft = withProfileChoice(draftFor(snippet({ profile: 'juniper_junos' })), ANY_PROFILE);

    expect(profileChoices(draft, null).map((choice) => choice.value)).toEqual([ANY_PROFILE, 'juniper_junos']);
  });
});
