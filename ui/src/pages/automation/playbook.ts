/**
 * Where a playbook's steps start.
 *
 * `playbooks.validate` numbers its complaints by step, and the editor marks
 * them in the gutter, so the screen has to know which line step 3 begins on.
 * The rule is deliberately dumb — a list item at the first indentation under
 * `steps:` — because guessing more than that about a document the backend
 * parses properly would only drift from it. A line it cannot place leaves the
 * problem without a marker; the message still says the step number.
 */

/** The 1-based line each top-level step starts on, in order. */
export function stepStartLines(yaml: string): number[] {
  const lines = yaml.split('\n');
  const starts: number[] = [];
  let inSteps = false;
  let indent: number | null = null;
  lines.forEach((line, index) => {
    if (/^steps\s*:/.test(line)) {
      inSteps = true;
      return;
    }
    if (!inSteps) return;
    if (line.trim() === '') return;
    const leading = line.length - line.trimStart().length;
    if (leading === 0 && !line.trimStart().startsWith('-')) {
      inSteps = false;
      return;
    }
    if (!line.trimStart().startsWith('- ')) return;
    if (indent === null) indent = leading;
    if (leading === indent) starts.push(index + 1);
  });
  return starts;
}
