/**
 * A template's variables as a form.
 *
 * The backend declares each variable's kind, so the form is built from the
 * declaration rather than from a guess: a `vlan` is a number between 1 and
 * 4094, a `choice` is a select, a `list` is a tag input. A value is only sent
 * when it was given — an empty optional variable is *absent*, not an empty
 * string, because `templates.render` is strict about undefined and would
 * rather name the missing variable than quietly render a blank line.
 *
 * Every value is one line, because the backend refuses a line break in one:
 * it would be a second line of configuration. The one exception is a
 * `string` declared `multiline` (a banner), which gets a text area.
 */

import type { Variable, VariableKind } from '@/api/types/automation';
import { Checkbox } from '@/design/Checkbox';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import styles from './Automation.module.css';
import { spansLines } from './values';

const PLACEHOLDER: Record<VariableKind, string> = {
  string: '',
  int: '0',
  bool: '',
  ip: '192.0.2.1',
  cidr: '192.0.2.0/24',
  vlan: '30',
  interface: 'GigabitEthernet0/1',
  list: 'Add one and press Enter',
  choice: '',
};

export interface VariablesFormProps {
  variables: readonly Variable[];
  value: Record<string, unknown>;
  onChange: (next: Record<string, unknown>) => void;
}

export function VariablesForm({ variables, value, onChange }: VariablesFormProps) {
  if (variables.length === 0) {
    return <p className={styles.hint}>This template takes no variables.</p>;
  }

  const set = (name: string, next: unknown) => {
    const copy = { ...value };
    if (next === undefined || next === '' || (Array.isArray(next) && next.length === 0)) delete copy[name];
    else copy[name] = next;
    onChange(copy);
  };

  return (
    <div className={styles.variables}>
      {variables.map((variable) => {
        const current = value[variable.name] ?? variable.default;
        const label = variable.label === '' ? variable.name : variable.label;

        if (variable.kind === 'bool') {
          return (
            <Checkbox
              key={variable.name}
              checked={current === true}
              onChange={(event) => set(variable.name, event.target.checked)}
            >
              {label}
              {variable.help !== '' && <span className={styles.hint}> — {variable.help}</span>}
            </Checkbox>
          );
        }

        if (variable.kind === 'choice') {
          return (
            <Field key={variable.name} label={label} hint={variable.help} required={variable.required}>
              <Select
                value={typeof current === 'string' ? current : ''}
                options={[
                  { value: '', label: variable.required ? 'Choose one…' : 'Not set' },
                  ...variable.choices.map((choice) => ({ value: choice, label: choice })),
                ]}
                onChange={(event) => set(variable.name, event.target.value)}
              />
            </Field>
          );
        }

        if (variable.kind === 'list') {
          return (
            <Field key={variable.name} label={label} hint={variable.help} required={variable.required}>
              <TagInput
                value={Array.isArray(current) ? current.map(String) : []}
                placeholder={PLACEHOLDER.list}
                onChange={(next) => set(variable.name, next)}
              />
            </Field>
          );
        }

        if (spansLines(variable)) {
          return (
            <Field key={variable.name} label={label} hint={variable.help} required={variable.required}>
              <Textarea
                rows={4}
                value={typeof current === 'string' ? current : ''}
                onChange={(event) => set(variable.name, event.target.value)}
              />
            </Field>
          );
        }

        const numeric = variable.kind === 'int' || variable.kind === 'vlan';
        return (
          <Field key={variable.name} label={label} hint={variable.help} required={variable.required}>
            <Input
              value={current === undefined || current === null ? '' : String(current)}
              placeholder={PLACEHOLDER[variable.kind]}
              inputMode={numeric ? 'numeric' : undefined}
              onChange={(event) => {
                const text = event.target.value;
                if (!numeric) return set(variable.name, text);
                const digits = text.replace(/[^0-9]/g, '');
                set(variable.name, digits === '' ? '' : Number(digits));
              }}
            />
          </Field>
        );
      })}
    </div>
  );
}

const KINDS: VariableKind[] = ['string', 'int', 'bool', 'ip', 'cidr', 'vlan', 'interface', 'list', 'choice'];

export interface DeclarationsProps {
  value: readonly Variable[];
  onChange: (next: Variable[]) => void;
  /** Names the body reads that are neither declared nor part of the context. */
  undeclared: readonly string[];
  disabled?: boolean;
}

/**
 * The variable *declarations* — what the form above is built from.
 *
 * `templates.validate` already says which names the body reads and cannot
 * provide, so the quickest honest path from a pasted body to a working
 * template is one button that declares exactly those, which can then be given
 * a kind and a label.
 */
export function Declarations({ value, onChange, undeclared, disabled = false }: DeclarationsProps) {
  const update = (index: number, patch: Partial<Variable>) =>
    onChange(value.map((variable, at) => (at === index ? { ...variable, ...patch } : variable)));

  return (
    <div className={styles.declarations}>
      {undeclared.length > 0 && !disabled && (
        <button
          type="button"
          className={styles.linkButton}
          onClick={() =>
            onChange([
              ...value,
              ...undeclared.map(
                (name): Variable => ({
                  name,
                  label: name.replace(/_/g, ' '),
                  kind: 'string',
                  required: true,
                  choices: [],
                  help: '',
                }),
              ),
            ])
          }
        >
          Declare {undeclared.join(', ')} as {undeclared.length === 1 ? 'a variable' : 'variables'}
        </button>
      )}
      {value.length > 0 && (
        <ul className={styles.declarationList} aria-label="Declared variables">
          {value.map((variable, index) => (
            <li key={variable.name}>
              <code>{variable.name}</code>
              <Select
                size="sm"
                aria-label={`Kind of ${variable.name}`}
                value={variable.kind}
                disabled={disabled}
                options={KINDS.map((kind) => ({ value: kind, label: kind }))}
                onChange={(event) => update(index, { kind: event.target.value as VariableKind })}
              />
              <Checkbox
                checked={variable.required}
                disabled={disabled}
                onChange={(event) => update(index, { required: event.target.checked })}
              >
                required
              </Checkbox>
              {variable.kind === 'string' && (
                <Checkbox
                  checked={variable.multiline === true}
                  disabled={disabled}
                  onChange={(event) => update(index, { multiline: event.target.checked })}
                >
                  several lines
                </Checkbox>
              )}
              {variable.kind === 'choice' && (
                <TagInput
                  value={variable.choices}
                  disabled={disabled}
                  placeholder="choices"
                  onChange={(choices) => update(index, { choices })}
                />
              )}
              {!disabled && (
                <button
                  type="button"
                  className={styles.linkButton}
                  onClick={() => onChange(value.filter((_entry, at) => at !== index))}
                >
                  Remove
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Which required variables have no value yet, by label. */
export function missingVariables(
  variables: readonly Variable[],
  value: Record<string, unknown>,
): string[] {
  return variables
    .filter((variable) => {
      if (!variable.required) return false;
      const current = value[variable.name] ?? variable.default;
      if (current === undefined || current === null || current === '') return true;
      return Array.isArray(current) && current.length === 0;
    })
    .map((variable) => (variable.label === '' ? variable.name : variable.label));
}

/** JSON an engineer typed, or `null` when it is not an object. */
export function varsFromText(text: string): Record<string, unknown> | null {
  if (text.trim() === '') return {};
  try {
    const parsed: unknown = JSON.parse(text);
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return null;
    return parsed as Record<string, unknown>;
  } catch {
    return null;
  }
}
