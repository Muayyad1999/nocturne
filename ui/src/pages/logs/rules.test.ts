import { describe, expect, it } from 'vitest';
import type { LogRule } from '@/api/types/logs';
import {
  RULE_BGP,
  RULE_COLD_START,
  RULE_LOGIN_BURST,
  RULE_OWN,
  RULE_TRAP_LINK_DOWN,
  TEST_THRESHOLD,
  TEST_TRAP,
  TEST_TRAP_CLEARS,
  TEST_TRAP_NO_MATCH,
} from './fixtures';
import {
  captureNames,
  clearingPhrase,
  draftOfRule,
  fieldOfRuleError,
  filterRules,
  lineSource,
  platformsOf,
  raisesPhrase,
  ruleParams,
  scopePhrase,
  testReading,
  thresholdPhrase,
  titlePlaceholders,
} from './rules';

const RULES: LogRule[] = [RULE_LOGIN_BURST, RULE_BGP, RULE_TRAP_LINK_DOWN, RULE_COLD_START, RULE_OWN];

describe('what is typed and what is sent', () => {
  it('sends a rule of one’s own back exactly as the engine stored it when nothing was changed', () => {
    const { params, errors } = ruleParams(draftOfRule(RULE_OWN));
    expect(errors).toEqual({});
    // Everything `logs.rules.save` reads, and nothing it does not own.
    const { builtin: _builtin, updated_at: _updated, ...sent } = RULE_OWN;
    expect(params).toEqual(sent);
  });

  it('sends the threshold and the quiet period as numbers', () => {
    const draft = { ...draftOfRule(), name: 'ACL denies', pattern: 'list (?P<acl>\\S+) denied' };
    const { params } = ruleParams({
      ...draft,
      thresholdOn: true,
      count: '3',
      window: '60',
      groupBy: ['acl'],
      clearAfter: '300',
      title: '{acl} is denying a lot',
    });
    expect(params.threshold).toEqual({ count: 3, window_s: 60, group_by: ['acl'] });
    expect(params.clear_after_s).toBe(300);
    expect(params.id).toBeUndefined();
  });

  it('sends no threshold when the switch is off, whatever the boxes still hold', () => {
    const { params } = ruleParams({ ...draftOfRule(RULE_OWN), thresholdOn: false });
    expect(params.threshold).toBeNull();
  });

  it('leaves the range of a number to the engine, and sends what was typed', () => {
    // 0 is a number. Whether it is a sensible count is the engine's to say,
    // in its own words — there is one set of rules, and it is not here.
    const { params, errors } = ruleParams({ ...draftOfRule(RULE_OWN), count: '0', clearAfter: '0' });
    expect(errors).toEqual({});
    expect(params.threshold?.count).toBe(0);
    expect(params.clear_after_s).toBe(0);
  });

  it('refuses only what cannot be sent as a number at all, beside its own field', () => {
    const { errors } = ruleParams({
      ...draftOfRule(RULE_OWN),
      count: 'eight',
      window: '-5',
      clearAfter: '1.5',
      throttle: '',
    });
    expect(Object.keys(errors).sort()).toEqual(['clear_after', 'count', 'throttle', 'window']);
  });

  it('says a number too large for the engine is too large, not that it is not written in digits', () => {
    const { errors } = ruleParams({ ...draftOfRule(RULE_OWN), count: '4294967296', throttle: '99999999999' });
    expect(errors.count).toMatch(/^Too large to send/);
    expect(errors.throttle).toMatch(/^Too large to send/);
    // The largest the engine holds is still sent, for the engine to judge.
    expect(ruleParams({ ...draftOfRule(RULE_OWN), throttle: '4294967295' }).errors).toEqual({});
  });

  it('sends nothing about clearing for a rule that raises no alert, whatever its hidden boxes still hold', () => {
    // An alert with an opposite and a quiet period, then changed to a reload
    // signal: the form hides both boxes, and the engine would refuse the rule
    // ("only a rule that raises an alert has something to clear") for text
    // that is no longer on the screen.
    const alert = { ...draftOfRule(RULE_OWN), clearPattern: 'permitted', clearAfter: '600' };
    expect(ruleParams(alert).params).toMatchObject({ clear_pattern: 'permitted', clear_after_s: 600 });
    const { params, errors } = ruleParams({ ...alert, emitKind: 'reload', clearAfter: 'not a number' });
    expect(errors).toEqual({});
    expect(params.clear_pattern).toBeNull();
    expect(params.clear_after_s).toBeNull();
  });

  it('treats an empty quiet period and an empty opposite as none, not as zero or as "matches everything"', () => {
    const { params } = ruleParams({ ...draftOfRule(RULE_OWN), clearAfter: ' ', clearPattern: '  ' });
    expect(params.clear_after_s).toBeNull();
    expect(params.clear_pattern).toBeNull();
  });

  it('puts no severity floor on a trap rule, because a trap has no severity to pass one', () => {
    const { params } = ruleParams({ ...draftOfRule(RULE_OWN), stream: 'trap', minSeverity: '3' });
    expect(params.min_severity).toBeNull();
    expect(ruleParams({ ...draftOfRule(RULE_OWN), minSeverity: '3' }).params.min_severity).toBe(3);
  });

  it('sends a built-in rule’s switch, scope and throttle and nothing of its definition', () => {
    const draft = { ...draftOfRule(RULE_LOGIN_BURST), enabled: false, throttle: '900' };
    const { params } = ruleParams({ ...draft, scope: { ...draft.scope, sites: ['HQ'] } });
    expect(params).toEqual({
      id: 'builtin.ios.login_burst',
      enabled: false,
      scope: { device_ids: [], sites: ['HQ'], roles: [], tags: [], profiles: [] },
      throttle_s: 900,
    });
  });

  it('shapes the signal by its kind', () => {
    expect(ruleParams({ ...draftOfRule(), emitKind: 'reload' }).params.emit).toEqual({ kind: 'reload' });
    expect(
      ruleParams({ ...draftOfRule(), emitKind: 'interface_down', interfaceGroup: ' port ' }).params.emit,
    ).toEqual({ kind: 'interface_down', interface_group: 'port' });
    expect(
      ruleParams({ ...draftOfRule(), emitKind: 'alert', severity: 'high', title: '{x} is down' }).params.emit,
    ).toEqual({ kind: 'alert', severity: 'high', title: '{x} is down' });
  });
});

describe('the engine’s words, beside the field they are about', () => {
  // Each sentence below is what `logs.rules.save` answered a running server,
  // or is quoted from `crates/nocturne-logs/src/rules/mod.rs`.
  it.each([
    ['a rule needs a name', 'name'],
    ['the pattern does not compile: regex parse error:\n    (unclosed\n    ^\nerror: unclosed group', 'pattern'],
    ['`urgent` is not a severity (info, low, medium, high, critical)', 'severity'],
    ['an alert needs a title (it may use the pattern\'s groups, as in `{interface} is down`)', 'title'],
    ['a threshold counts between 1 and 1000 lines, not 0', 'count'],
    ["a threshold's window is between 1 and 86400 seconds, not 0", 'window'],
    ['the pattern has no group named `nope` to count by', 'group_by'],
    ['the clearing pattern does not compile: regex parse error', 'clear_pattern'],
    ['the clearing pattern has to capture `acl` as well, or it cannot tell which alert to clear', 'clear_pattern'],
    ['only a rule that raises an alert has something to clear', 'clear_pattern'],
    ['an alert is cleared after between 1 and 2592000 quiet seconds, not 0', 'clear_after'],
    ['a rule cannot follow itself', 'after'],
    ['`IOS: interface up` raises no alert, so there is nothing for another rule to follow', 'after'],
    ['there is no rule `gone` to follow', 'after'],
  ] as const)('places “%s” beside %s', (message, field) => {
    expect(fieldOfRuleError(message)).toBe(field);
  });

  it('puts a refusal it does not recognise above the form, whole, rather than guessing a field', () => {
    expect(
      fieldOfRuleError('invalid parameters for `logs.rules.save`: invalid type: string "x", expected u32'),
    ).toBe('form');
    expect(fieldOfRuleError('the database is locked')).toBe('form');
  });
});

describe('the groups a pattern captures', () => {
  it('reads the engine’s own spelling and the other one', () => {
    expect(captureNames(RULE_LOGIN_BURST.pattern)).toEqual(['user', 'source']);
    expect(captureNames('(?<a>x)(?P<b>y)')).toEqual(['a', 'b']);
  });

  it('is not fooled by an escaped parenthesis, a character class or a lookbehind', () => {
    expect(captureNames('\\(?P<no>x\\)')).toEqual([]);
    expect(captureNames('[(?P<no>](?P<yes>z)')).toEqual(['yes']);
    expect(captureNames('(?<=x)(?<!y)(?:z)(?P<real>w)')).toEqual(['real']);
  });

  it('names a group once however often the pattern spells it', () => {
    expect(captureNames(RULE_TRAP_LINK_DOWN.pattern)).toEqual(['ifindex', 'name']);
    expect(captureNames('(?P<a>x)|(?P<a>y)')).toEqual(['a']);
  });

  it('still names the groups of a pattern that does not compile', () => {
    expect(captureNames('(?P<acl>\\S+ (unclosed')).toEqual(['acl']);
  });

  it('finds the placeholders of a title', () => {
    expect(titlePlaceholders('Link down: ifIndex {ifindex}{name}')).toEqual(['ifindex', 'name']);
    expect(titlePlaceholders('No placeholders {} here { x }')).toEqual([]);
  });
});

describe('a rule in a phrase', () => {
  it('says how many in how long, per what', () => {
    expect(thresholdPhrase(RULE_LOGIN_BURST)).toBe('8 in 60 s, per source');
    expect(thresholdPhrase(RULE_OWN)).toBe('3 in 60 s, per acl');
  });

  it('reads a threshold of one as "every line", per group — one instance per neighbour', () => {
    expect(thresholdPhrase(RULE_BGP)).toBe('Every match, per neighbor');
    expect(thresholdPhrase(RULE_COLD_START)).toBe('Every match');
  });

  it('says what ends the alert: its opposite, a quiet period, or a person', () => {
    expect(clearingPhrase(RULE_BGP)).toBe('Its opposite event');
    expect(clearingPhrase(RULE_LOGIN_BURST)).toBe('600 s quiet');
    expect(clearingPhrase({ ...RULE_BGP, clear_after_s: 900 })).toBe('Its opposite, or 900 s quiet');
    expect(clearingPhrase({ ...RULE_BGP, clear_pattern: null })).toBe('By hand, in the alert list');
  });

  it('does not talk of clearing for a rule that raises no alert', () => {
    expect(clearingPhrase(RULE_COLD_START)).toBe('Raises no alert');
    expect(raisesPhrase(RULE_COLD_START.emit)).toBe('Reload');
    expect(raisesPhrase(RULE_LOGIN_BURST.emit)).toBe('Alert · high');
  });

  it('says which devices a scope narrows to, and "every device" when it does not', () => {
    expect(scopePhrase(RULE_OWN.scope)).toBe('Every device');
    expect(
      scopePhrase({ device_ids: ['a', 'b'], sites: ['HQ'], roles: [], tags: ['critical', 'edge'], profiles: [] }),
    ).toBe('site HQ · tags critical, edge · 2 chosen devices');
  });
});

describe('where a rule’s test line came from', () => {
  it('reads "synthetic" from the rule’s own statement and never upgrades it', () => {
    const login = lineSource(RULE_LOGIN_BURST.test_line_source);
    expect(login.kind).toBe('synthetic');
    expect(login.text).toBe(RULE_LOGIN_BURST.test_line_source);
    // A trap rule's line is synthetic in another way, and says so itself.
    expect(lineSource(RULE_TRAP_LINK_DOWN.test_line_source).kind).toBe('synthetic');
    expect(RULE_TRAP_LINK_DOWN.test_line_source).toContain('net-snmp sent it, not a device');
  });

  it('tells a quoted line from one that only names its documentation', () => {
    expect(lineSource('Quoted from Cisco, "Configure SSH on Routers", Document ID 4145').kind).toBe('quoted');
    expect(lineSource(RULE_BGP.test_line_source)).toMatchObject({
      kind: 'stated',
      label: 'From documentation',
    });
  });

  it('says so when a rule does not state where its line is from', () => {
    expect(lineSource(RULE_OWN.test_line_source)).toMatchObject({ kind: 'none', label: 'Source not stated' });
  });
});

describe('what the test box says', () => {
  it('does not call a match that needs two more lines a raise', () => {
    const reading = testReading(TEST_THRESHOLD, 'syslog');
    expect(reading.tone).toBe('warn');
    expect(reading.headline).toContain('raises nothing yet');
    // The engine's own note, unchanged.
    expect(reading.sentences).toEqual([TEST_THRESHOLD.note]);
    expect(reading.facts).toContainEqual({ label: 'Counted under', value: 'EDGE-IN', mono: true });
    expect(reading.facts).toContainEqual({ label: 'Severity', value: 'informational (6)' });
  });

  it('reads a trap rule’s answer as a trap: a notification, and no severity', () => {
    const reading = testReading(TEST_TRAP, 'trap');
    expect(reading).toMatchObject({ tone: 'ok', headline: 'The rule matches, and this line raises its signal' });
    expect(reading.facts).toContainEqual({ label: 'Notification', value: 'linkDown', mono: true });
    expect(reading.facts).toContainEqual({ label: 'Severity', value: 'none — a trap carries no severity' });
    expect(reading.facts).toContainEqual({
      label: 'It would raise',
      value: 'an alert (low): Link down: ifIndex 3 (GigabitEthernet0/3)',
    });
    expect(reading.facts).toContainEqual({ label: 'Counted under', value: '3', mono: true });
  });

  it('says a line is the rule’s opposite, with the engine’s reason', () => {
    const reading = testReading(TEST_TRAP_CLEARS, 'trap');
    expect(reading.headline).toBe('This is the rule’s opposite: it clears the alert');
    expect(reading.sentences).toEqual([TEST_TRAP_CLEARS.reason]);
    expect(reading.facts).toContainEqual({ label: 'Counted under', value: '3', mono: true });
  });

  it('says a line does not match, and does not say what it would be counted under', () => {
    const reading = testReading(TEST_TRAP_NO_MATCH, 'trap');
    expect(reading).toMatchObject({ tone: 'idle', headline: 'The rule does not match this line' });
    expect(reading.sentences).toEqual(['the pattern does not match this line']);
    expect(reading.facts.map((fact) => fact.label)).not.toContain('Counted under');
  });

  it('tells a syslog line that came without a priority from one of a known severity', () => {
    const reading = testReading(
      { ...TEST_THRESHOLD, parsed: { ...TEST_THRESHOLD.parsed, severity: null, message: 'x' } },
      'syslog',
    );
    expect(reading.facts).toContainEqual({
      label: 'Severity',
      value: 'none given — the line came without a priority',
    });
  });
});

describe('the rule list', () => {
  it('lists the platforms the rules are for, without the user’s own', () => {
    expect(platformsOf(RULES)).toEqual(['Any SNMP agent (SNMPv2-MIB, IF-MIB)', 'Cisco IOS / IOS-XE']);
  });

  it('narrows by platform, by stream and to the user’s own rules', () => {
    const by = (filter: Partial<Parameters<typeof filterRules>[1]>) =>
      filterRules(RULES, { text: '', platform: '', stream: '', ...filter }).map((rule) => rule.id);
    expect(by({ stream: 'trap' })).toEqual(['builtin.trap.link_down', 'builtin.trap.cold_start']);
    expect(by({ platform: 'own' })).toEqual([RULE_OWN.id]);
    expect(by({ platform: 'Cisco IOS / IOS-XE' })).toEqual(['builtin.ios.login_burst', 'builtin.ios.bgp_adjacency']);
  });

  it('finds a rule by a mnemonic in its pattern as well as by its name', () => {
    const found = filterRules(RULES, { text: 'sec_login', platform: '', stream: '' });
    expect(found.map((rule) => rule.id)).toEqual(['builtin.ios.login_burst']);
    expect(filterRules(RULES, { text: 'acl denies', platform: '', stream: '' })).toEqual([RULE_OWN]);
  });
});
