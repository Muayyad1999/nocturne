/**
 * A tool's form, drawn from its {@link Field} list.
 *
 * Every tool's inputs are declared in `specs.ts`, so there is one form
 * component rather than forty: labels, hints, monospace where a device's words
 * go, and the one rule that matters for a calculator — nothing is submitted,
 * because there is nothing to submit. Typing *is* the question.
 *
 * A field with a `when` is shown only while the field it depends on has that
 * value, which is how "aim at a host or at a device" stays one choice instead
 * of two boxes that contradict each other.
 *
 * A choice whose options are the engine's own table (`Field.source`) is read
 * here, so every form gets it without being told: the options are whatever
 * the table holds, "still reading" is not shown as "nothing to choose", and a
 * table that could not be read says so in the engine's words.
 *
 * A refusal the engine aimed at one input (`errors`) is shown beside that
 * input, in the place its hint was, and marks the control invalid.
 */

import { Combobox } from '@/design/Combobox';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Switch } from '@/design/Switch';
import { Textarea } from '@/design/Textarea';
import type { DeviceRow } from '@/api/types/core';
import { useTables, type TableRead } from './data';
import { fieldShown, pickerOf, type Field as FieldSpec, type Values } from './specs';
import styles from './Tools.module.css';

export interface FieldsProps {
  fields: readonly FieldSpec[];
  values: Values;
  onChange: (name: string, value: string) => void;
  /** The inventory, for a `device` field. */
  devices?: readonly DeviceRow[];
  /** Enter in a single-line field: run the tool that has a button. */
  onSubmit?: () => void;
  disabled?: boolean;
  /** The engine's refusal, by the name of the field it is about. */
  errors?: Readonly<Record<string, string>>;
}

export function Fields({ fields, values, onChange, devices, onSubmit, disabled, errors }: FieldsProps) {
  const tables = useTables(fields);
  const shown = fields.filter((field) => fieldShown(field, values));
  if (shown.length === 0) return null;

  return (
    <div className={styles.form}>
      {shown.map((field) => (
        <div key={field.name} className={styles.formField} data-span={field.span ?? 1}>
          <OneField
            field={field}
            value={values[field.name] ?? ''}
            onChange={(value) => onChange(field.name, value)}
            devices={devices}
            onSubmit={onSubmit}
            disabled={disabled}
            error={errors?.[field.name]}
            table={field.source ? tables[field.source.table] : undefined}
          />
        </div>
      ))}
    </div>
  );
}

interface OneFieldProps {
  field: FieldSpec;
  value: string;
  onChange: (value: string) => void;
  devices?: readonly DeviceRow[];
  onSubmit?: () => void;
  disabled?: boolean;
  error?: string;
  /** The engine's table behind this field's choices, when it has one. */
  table?: TableRead;
}

function OneField({ field, value, onChange, devices, onSubmit, disabled, error, table }: OneFieldProps) {
  const enter = (event: { key: string; preventDefault: () => void }) => {
    if (event.key === 'Enter' && onSubmit) {
      event.preventDefault();
      onSubmit();
    }
  };

  if (field.kind === 'switch') {
    // A switch has no Field around it, so a refusal about it is wired to it
    // by hand: the same sentence, in the same place under the control.
    const errorId = `tool-field-${field.name}-error`;
    return (
      <>
        <Switch
          checked={value === 'on'}
          disabled={disabled}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errorId : undefined}
          onChange={(event) => onChange(event.currentTarget.checked ? 'on' : '')}
        >
          {field.label}
        </Switch>
        {error && (
          <p id={errorId} className={styles.switchError}>
            {error}
          </p>
        )}
      </>
    );
  }

  if (field.kind === 'textarea') {
    return (
      <Field label={field.label} hint={field.hint} error={error}>
        <Textarea
          value={value}
          rows={field.rows ?? 4}
          mono
          spellCheck={false}
          placeholder={field.placeholder}
          disabled={disabled}
          onChange={(event) => onChange(event.currentTarget.value)}
        />
      </Field>
    );
  }

  if (field.kind === 'select') {
    const picker = pickerOf(field, value, table);
    return (
      <Field label={field.label} hint={picker.hint} error={error ?? picker.error}>
        <Select
          value={value}
          options={picker.options}
          disabled={disabled || !picker.ready}
          onChange={(event) => onChange(event.currentTarget.value)}
        />
      </Field>
    );
  }

  if (field.kind === 'device') {
    const options = (devices ?? []).map((device) => ({
      value: device.name,
      label: device.lab ? `${device.name} · lab` : device.name,
      description: device.address || 'no address in the inventory',
    }));
    return (
      <Field label={field.label} hint={field.hint} error={error}>
        <Combobox
          options={options}
          value={value === '' ? null : value}
          clearable
          disabled={disabled}
          placeholder={devices === undefined ? 'Reading the inventory…' : 'Search devices…'}
          emptyText="No device of that name"
          onChange={(next) => onChange(next ?? '')}
        />
      </Field>
    );
  }

  return (
    <Field label={field.label} hint={field.hint} error={error}>
      <Input
        value={value}
        type={field.kind === 'number' ? 'number' : 'text'}
        inputMode={field.kind === 'number' ? 'numeric' : undefined}
        mono={field.kind === 'mono'}
        spellCheck={false}
        autoComplete="off"
        placeholder={field.placeholder}
        disabled={disabled}
        onKeyDown={enter}
        onChange={(event) => onChange(event.currentTarget.value)}
      />
    </Field>
  );
}
