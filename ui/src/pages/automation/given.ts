/**
 * Did the backend actually give us this?
 *
 * The types in `api/types/automation.ts` write an optional field as `?`, and
 * say a field written that way is genuinely missing rather than `null`. On the
 * wire it is both: `CommandResult.parsed` really is absent, while `Run.caveat`,
 * `Run.task_id`, `DeviceResult.reason`, `Schedule.next_run_at`,
 * `TemplateProblem.line` and the rest arrive as an explicit `null`. A
 * `!== undefined` test therefore passes for a field the backend said it had
 * nothing for — which is how a plain command run came to show the Ansible
 * caveat line, and how a filter with no lab preference came to read as "real
 * devices only".
 *
 * So: one guard, used wherever an optional backend field decides what is on
 * screen.
 */

/** True when `value` is neither absent nor null — and narrows it. */
export function given<T>(value: T | undefined): value is T {
  return value !== undefined && value !== null;
}

/** The value, with the backend's `null` folded into "not given". */
export function orNone<T>(value: T | undefined): T | undefined {
  return given(value) ? value : undefined;
}
