/**
 * What a template variable takes, as the backend decides it.
 *
 * Every value is one line: the backend refuses a line break in one, because
 * it would be a second line of configuration, and it refuses any other
 * control character with it. The one exception is a `string` declared
 * `multiline` (a banner), which the form gives a text area.
 */

import type { Variable } from '@/api/types/automation';

/** Does this variable take text over several lines? */
export function spansLines(variable: Pick<Variable, 'kind' | 'multiline'>): boolean {
  return variable.kind === 'string' && variable.multiline === true;
}
