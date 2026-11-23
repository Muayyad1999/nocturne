/**
 * The reasoning behind the rule list and the rule editor.
 *
 * A log rule turns lines into a signal: a pattern, and optionally how many
 * matches inside how long (a threshold), under which captures they are
 * counted, and what ends the alert again (its opposite event, or a quiet
 * period). The engine owns every judgement about whether a rule can work —
 * `logs.rules.save` answers `invalid` with the reason — so this module does
 * three smaller things and no more:
 *
 * * **It keeps what is typed apart from what is sent.** The form holds text
 *   ({@link RuleDraft}); {@link ruleParams} turns it into the engine's
 *   shape. The only thing refused here is text that cannot be sent as a
 *   number at all. Whether 0 is a sensible count is the engine's to say, in
 *   its own words.
 * * **It puts the engine's words beside the field they are about.**
 *   {@link fieldOfRuleError} reads a refusal and names the field; a message
 *   it does not recognise goes above the form, whole. It never rewrites one.
 * * **It says what a rule does in a phrase** for the list: how many in how
 *   long per what, and what clears it.
 *
 * A built-in rule's definition is the application's. Only its switch, its
 * scope and its throttle are the user's, and {@link ruleParams} sends
 * exactly those three for one.
 */

import type {
  LogRule,
  RuleSaveParams,
  RuleScope,
  RuleTest,
  Signal,
  SignalKind,
  Threshold,
} from '@/api/types/logs';
import type { StatusTone } from '@/design/tone';
import { severityName } from './query';

export const ALERT_SEVERITIES = ['info', 'low', 'medium', 'high', 'critical'] as const;

export const SIGNAL_KINDS: readonly { value: SignalKind; label: string; help: string }[] = [
  { value: 'alert', label: 'An alert', help: 'Raised in the alert list, with a severity and a title.' },
  { value: 'config_changed', label: 'Configuration changed', help: 'Tells the configuration feature to back the device up.' },
  { value: 'interface_down', label: 'Interface down', help: 'A signal that names the interface the pattern captured.' },
  { value: 'interface_up', label: 'Interface up', help: 'A signal that names the interface the pattern captured.' },
  { value: 'login_failure', label: 'Login failure', help: 'A signal; no alert by itself.' },
  { value: 'reload', label: 'Reload', help: 'The device restarted. The detections compare it with what Nocturne was doing.' },
];

export const RULE_STREAMS: readonly { value: string; label: string; help: string }[] = [
  { value: 'syslog', label: 'Syslog lines', help: 'The pattern is matched against the line as the device sent it.' },
  {
    value: 'trap',
    label: 'SNMP traps',
    help: 'The pattern is matched against the message the receiver writes for a trap: its name, then its bindings by name.',
  },
];

/** What the editor holds while it is typed in. Numbers are text until they are sent. */
export interface RuleDraft {
  /** Empty for a rule that does not exist yet. */
  id: string;
  builtin: boolean;
  name: string;
  enabled: boolean;
  platform: string;
  stream: string;
  pattern: string;
  /** `''` is "any severity". */
  minSeverity: string;
  scope: RuleScope;
  emitKind: SignalKind;
  /** For an alert. */
  severity: string;
  title: string;
  /** For an interface signal: the capture that holds the interface. */
  interfaceGroup: string;
  thresholdOn: boolean;
  count: string;
  window: string;
  groupBy: string[];
  /** Another rule's id, or `''`. */
  after: string;
  clearPattern: string;
  /** Seconds, or `''` for "no quiet period". */
  clearAfter: string;
  throttle: string;
  testLine: string;
  testLineSource: string;
}

const EMPTY_SCOPE: RuleScope = { device_ids: [], sites: [], roles: [], tags: [], profiles: [] };

/** A rule as the form's text; with no rule, the engine's own defaults. */
export function draftOfRule(rule?: LogRule): RuleDraft {
  const emit: Signal = rule?.emit ?? { kind: 'alert', severity: 'medium', title: '' };
  const threshold = rule?.threshold ?? null;
  return {
    id: rule?.id ?? '',
    builtin: rule?.builtin ?? false,
    name: rule?.name ?? '',
    enabled: rule?.enabled ?? true,
    platform: rule?.platform ?? '',
    stream: rule?.stream ?? 'syslog',
    pattern: rule?.pattern ?? '',
    minSeverity: rule?.min_severity == null ? '' : String(rule.min_severity),
    scope: { ...EMPTY_SCOPE, ...(rule?.scope ?? {}) },
    emitKind: emit.kind,
    severity: emit.kind === 'alert' ? emit.severity : 'medium',
    title: emit.kind === 'alert' ? emit.title : '',
    interfaceGroup:
      emit.kind === 'interface_down' || emit.kind === 'interface_up' ? emit.interface_group : 'interface',
    thresholdOn: threshold !== null,
    count: threshold === null ? '5' : String(threshold.count),
    window: threshold === null ? '60' : String(threshold.window_s),
    groupBy: threshold === null ? [] : [...threshold.group_by],
    after: rule?.after ?? '',
    clearPattern: rule?.clear_pattern ?? '',
    clearAfter: rule?.clear_after_s == null ? '' : String(rule.clear_after_s),
    throttle: String(rule?.throttle_s ?? 60),
    testLine: rule?.test_line ?? '',
    testLineSource: rule?.test_line_source ?? '',
  };
}

export type RuleField =
  | 'name'
  | 'pattern'
  | 'severity'
  | 'title'
  | 'count'
  | 'window'
  | 'group_by'
  | 'after'
  | 'clear_pattern'
  | 'clear_after'
  | 'throttle'
  /** Not about one field: shown above the form. */
  | 'form';

export type RuleErrors = Partial<Record<RuleField, string>>;

/** The largest number the engine's fields hold. */
const U32_MAX = 4_294_967_295;

const NOT_A_NUMBER = 'A whole number, written in digits.';
// The engine would refuse it in the words of its parameter parser, above the
// form: said here, by the field, and without claiming a range that is the
// engine's to state.
const TOO_LARGE = `Too large to send: the engine keeps this number in 32 bits (at most ${U32_MAX}).`;

function whole(typed: string): { value: number; error: null } | { value: null; error: string } {
  const trimmed = typed.trim();
  if (!/^\d+$/.test(trimmed)) return { value: null, error: NOT_A_NUMBER };
  const value = Number(trimmed);
  return value <= U32_MAX ? { value, error: null } : { value: null, error: TOO_LARGE };
}

function emitOf(draft: RuleDraft): Signal {
  switch (draft.emitKind) {
    case 'alert':
      return { kind: 'alert', severity: draft.severity, title: draft.title };
    case 'interface_down':
    case 'interface_up':
      return { kind: draft.emitKind, interface_group: draft.interfaceGroup.trim() };
    default:
      return { kind: draft.emitKind };
  }
}

/**
 * What to hand to `logs.rules.save` (and, as `rule`, to `logs.rules.test`).
 *
 * `errors` is only ever about text that is not a number: everything a rule
 * can be wrong about beyond that is left for the engine to refuse, so that
 * there is one set of rules and one set of words for them.
 */
export function ruleParams(draft: RuleDraft): { params: RuleSaveParams; errors: RuleErrors } {
  const errors: RuleErrors = {};
  const throttle = whole(draft.throttle);
  if (throttle.error !== null) errors.throttle = throttle.error;

  if (draft.builtin) {
    // The definition is the application's: these three are all that is read.
    return {
      params: {
        id: draft.id,
        enabled: draft.enabled,
        scope: draft.scope,
        throttle_s: throttle.value ?? 60,
      },
      errors,
    };
  }

  let threshold: Threshold | null = null;
  if (draft.thresholdOn) {
    const count = whole(draft.count);
    const window = whole(draft.window);
    if (count.error !== null) errors.count = count.error;
    if (window.error !== null) errors.window = window.error;
    threshold = { count: count.value ?? 0, window_s: window.value ?? 0, group_by: draft.groupBy };
  }

  // What ends an alert means nothing for a rule that raises none, and the
  // form hides those boxes then. What they still hold from before the kind
  // was changed is kept for a change back, and not sent: the engine would
  // refuse the rule for something that is not on the screen.
  const raisesAlert = draft.emitKind === 'alert';
  let clearAfter: number | null = null;
  if (raisesAlert && draft.clearAfter.trim() !== '') {
    const typed = whole(draft.clearAfter);
    clearAfter = typed.value;
    if (typed.error !== null) errors.clear_after = typed.error;
  }

  return {
    params: {
      ...(draft.id === '' ? {} : { id: draft.id }),
      name: draft.name.trim(),
      enabled: draft.enabled,
      platform: draft.platform,
      stream: draft.stream,
      pattern: draft.pattern,
      // A trap has no severity, so a floor on one would match nothing.
      min_severity: draft.stream === 'trap' || draft.minSeverity === '' ? null : Number(draft.minSeverity),
      scope: draft.scope,
      threshold,
      after: draft.after === '' ? null : draft.after,
      clear_pattern: !raisesAlert || draft.clearPattern.trim() === '' ? null : draft.clearPattern,
      clear_after_s: clearAfter,
      emit: emitOf(draft),
      throttle_s: throttle.value ?? 60,
      test_line: draft.testLine,
      test_line_source: draft.testLineSource,
    },
    errors,
  };
}

/**
 * Which field one of the engine's refusals is about.
 *
 * These are the engine's sentences (`crates/nocturne-logs/src/rules/mod.rs`,
 * `check`), recognised by how they begin so that the message itself can be
 * shown unchanged beside the field. One that is not recognised — a newer
 * engine, a parameter of the wrong type — is about the form as a whole.
 */
export function fieldOfRuleError(message: string): RuleField {
  const text = message.trim();
  if (text.startsWith('a rule needs a name')) return 'name';
  if (text.startsWith('the pattern does not compile')) return 'pattern';
  if (text.includes('is not a severity')) return 'severity';
  if (text.startsWith('an alert needs a title')) return 'title';
  if (text.startsWith('a threshold counts between')) return 'count';
  if (text.startsWith("a threshold's window is between")) return 'window';
  if (/^the pattern has no group named .* to count by/.test(text)) return 'group_by';
  if (text.startsWith('the clearing pattern')) return 'clear_pattern';
  if (text.startsWith('an alert is cleared after between')) return 'clear_after';
  if (text.startsWith('only a rule that raises an alert has something to clear')) return 'clear_pattern';
  if (
    text.startsWith('a rule cannot follow itself') ||
    text.includes('so there is nothing for another rule to follow') ||
    /^there is no rule .* to follow/.test(text)
  ) {
    return 'after';
  }
  return 'form';
}

/**
 * The named groups a pattern captures, in order, for the "count by" choices.
 *
 * A scanner rather than a regular expression over a regular expression: an
 * escaped parenthesis and a `(` inside a character class open no group, and
 * `(?<=` / `(?<!` are lookbehinds, not names. It reads `(?P<name>` (the
 * engine's own spelling) and `(?<name>` alike. A pattern that does not
 * compile still yields the names it spells — the engine says what is wrong
 * with it.
 */
export function captureNames(pattern: string): string[] {
  const names: string[] = [];
  let inClass = false;
  for (let index = 0; index < pattern.length; index += 1) {
    const character = pattern[index];
    if (character === '\\') {
      index += 1;
      continue;
    }
    if (inClass) {
      if (character === ']') inClass = false;
      continue;
    }
    if (character === '[') {
      inClass = true;
      continue;
    }
    if (character !== '(' || pattern[index + 1] !== '?') continue;
    const rest = pattern.slice(index + 2);
    const found = /^P?<([A-Za-z_][A-Za-z0-9_]*)>/.exec(rest);
    if (found?.[1] && !names.includes(found[1])) names.push(found[1]);
  }
  return names;
}

/** `{group}` placeholders in an alert's title, in order, once each. */
export function titlePlaceholders(title: string): string[] {
  const names: string[] = [];
  for (const found of title.matchAll(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g)) {
    if (found[1] && !names.includes(found[1])) names.push(found[1]);
  }
  return names;
}

// ------------------------------------------------------------------ phrases

/** `60` → `60 s`. The engine counts in seconds and so does the screen. */
function seconds(value: number): string {
  return `${value} s`;
}

/**
 * How many in how long, per what: `8 in 60 s, per source`. A threshold of
 * one is "every match", counted per group — one instance per neighbour.
 */
export function thresholdPhrase(rule: Pick<LogRule, 'threshold'>): string {
  const threshold = rule.threshold;
  if (threshold == null) return 'Every match';
  const per = threshold.group_by.length === 0 ? '' : `, per ${threshold.group_by.join(' and ')}`;
  if (threshold.count <= 1) return `Every match${per}`;
  return `${threshold.count} in ${seconds(threshold.window_s)}${per}`;
}

/** What ends the alert a rule raised. */
export function clearingPhrase(rule: Pick<LogRule, 'emit' | 'clear_pattern' | 'clear_after_s'>): string {
  if (rule.emit.kind !== 'alert') return 'Raises no alert';
  const opposite = rule.clear_pattern != null && rule.clear_pattern !== '';
  const quiet = rule.clear_after_s != null;
  if (opposite && quiet) return `Its opposite, or ${seconds(rule.clear_after_s as number)} quiet`;
  if (opposite) return 'Its opposite event';
  if (quiet) return `${seconds(rule.clear_after_s as number)} quiet`;
  return 'By hand, in the alert list';
}

/** What a rule raises, in a word or two. */
export function raisesPhrase(emit: Signal): string {
  if (emit.kind === 'alert') return `Alert · ${emit.severity}`;
  return SIGNAL_KINDS.find((kind) => kind.value === emit.kind)?.label ?? emit.kind;
}

export function alertTone(severity: string | null | undefined): StatusTone {
  switch (severity) {
    case 'critical':
    case 'high':
      return 'crit';
    case 'medium':
      return 'warn';
    case 'low':
    case 'info':
      return 'info';
    default:
      return 'idle';
  }
}

export interface LineSource {
  /**
   * `quoted`: the rule says the line is quoted from documentation.
   * `synthetic`: written by hand to a documented format, or (a trap rule)
   * the receiver's own message for a datagram a tool sent.
   * `stated`: the rule names where it came from and claims neither.
   * `none`: the rule gives no line, or does not say where it is from.
   */
  kind: 'quoted' | 'synthetic' | 'stated' | 'none';
  /** The badge. */
  label: string;
  /** The engine's own sentence, unchanged. */
  text: string;
}

/**
 * Where a rule's test line came from, read from the rule's own statement.
 * The engine marks a hand-written line `Synthetic: …`; this never upgrades
 * one to "recorded" — no built-in line was recorded from a device.
 */
export function lineSource(source: string): LineSource {
  const text = source.trim();
  if (text === '') return { kind: 'none', label: 'Source not stated', text };
  if (/^synthetic\b/i.test(text)) return { kind: 'synthetic', label: 'Synthetic', text };
  if (/^quoted\b/i.test(text)) return { kind: 'quoted', label: 'Quoted from documentation', text };
  return { kind: 'stated', label: 'From documentation', text };
}

// ----------------------------------------------------------------- the test

export interface TestReading {
  tone: StatusTone;
  /** The verdict in a few words. */
  headline: string;
  /** The engine's sentences, each unchanged: its reason, and its note. */
  sentences: string[];
  /** What was read out of the line, as label and text. */
  facts: { label: string; value: string; mono?: boolean }[];
}

/**
 * What `logs.rules.test` said, arranged for reading. Nothing is added to the
 * engine's answer but labels: a matching line that raises nothing by itself
 * (a threshold wants more of them) carries the engine's `note`, and is not
 * dressed up as "this raises the alert".
 */
export function testReading(result: RuleTest, stream: string): TestReading {
  const sentences: string[] = [];
  if (result.reason !== '') sentences.push(result.reason);
  if (result.note !== '') sentences.push(result.note);

  const facts: TestReading['facts'] = [];
  if (result.signal) {
    facts.push({
      label: 'It would raise',
      value:
        result.signal.kind === 'alert'
          ? `an alert${result.signal.severity ? ` (${result.signal.severity})` : ''}: ${result.signal.title}`
          : `${result.signal.kind}${result.signal.interface ? ` on ${result.signal.interface}` : ''}`,
    });
  }
  if (result.matched || result.clears) {
    facts.push({
      label: 'Counted under',
      value: result.instance === '' ? 'the device as a whole' : result.instance,
      mono: result.instance !== '',
    });
  }
  for (const [name, value] of Object.entries(result.groups)) {
    facts.push({ label: `Captured ${name}`, value: value === '' ? '(nothing)' : value, mono: value !== '' });
  }
  const parsed = result.parsed;
  if (parsed) {
    if (stream === 'trap') {
      facts.push({
        label: 'Notification',
        value: parsed.mnemonic ?? 'the line names none',
        mono: parsed.mnemonic != null,
      });
      facts.push({ label: 'Severity', value: 'none — a trap carries no severity' });
    } else {
      facts.push({ label: 'Mnemonic', value: parsed.mnemonic ?? 'none in the line', mono: parsed.mnemonic != null });
      facts.push({
        label: 'Severity',
        value:
          parsed.severity == null
            ? 'none given — the line came without a priority'
            : `${severityName(parsed.severity)} (${parsed.severity})`,
      });
    }
  }

  if (result.clears) {
    return { tone: 'info', headline: 'This is the rule’s opposite: it clears the alert', sentences, facts };
  }
  if (!result.matched) {
    return { tone: 'idle', headline: 'The rule does not match this line', sentences, facts };
  }
  // A match that needs more before anything is raised is not a raise.
  return result.note === ''
    ? { tone: 'ok', headline: 'The rule matches, and this line raises its signal', sentences, facts }
    : { tone: 'warn', headline: 'The rule matches — and this one line raises nothing yet', sentences, facts };
}

// ----------------------------------------------------------------- the list

export interface RuleFilter {
  text: string;
  /** `''` every platform; `own` the user's rules; otherwise a platform. */
  platform: string;
  /** `''` either stream. */
  stream: string;
}

/** The platforms the rules are for, as the list's filter offers them. */
export function platformsOf(rules: readonly LogRule[]): string[] {
  return [...new Set(rules.map((rule) => rule.platform).filter((platform) => platform !== ''))].sort();
}

export function filterRules(rules: readonly LogRule[], filter: RuleFilter): LogRule[] {
  const needle = filter.text.trim().toLowerCase();
  return rules.filter((rule) => {
    if (filter.stream !== '' && rule.stream !== filter.stream) return false;
    if (filter.platform === 'own' && rule.builtin) return false;
    if (filter.platform !== '' && filter.platform !== 'own' && rule.platform !== filter.platform) return false;
    if (needle === '') return true;
    return [rule.name, rule.pattern, rule.platform, rule.id].some((field) =>
      field.toLowerCase().includes(needle),
    );
  });
}

/** How many devices a scope narrows to, in words; "every device" when it does not narrow. */
export function scopePhrase(scope: RuleScope): string {
  const parts: string[] = [];
  const add = (values: readonly string[], one: string, many: string) => {
    if (values.length > 0) parts.push(`${values.length === 1 ? one : many} ${values.join(', ')}`);
  };
  add(scope.sites, 'site', 'sites');
  add(scope.roles, 'role', 'roles');
  add(scope.tags, 'tag', 'tags');
  add(scope.profiles, 'profile', 'profiles');
  if (scope.device_ids.length > 0) {
    parts.push(`${scope.device_ids.length} chosen ${scope.device_ids.length === 1 ? 'device' : 'devices'}`);
  }
  return parts.length === 0 ? 'Every device' : parts.join(' · ');
}
