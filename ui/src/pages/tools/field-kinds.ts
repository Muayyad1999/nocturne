/**
 * What a tool's form is made of. Types only, so `specs.ts` stays free of
 * React and can be tested as arithmetic.
 */

export type FieldKind =
  /** A line of prose. */
  | 'text'
  /** A line of something a device said or will be told: monospace. */
  | 'mono'
  | 'number'
  | 'textarea'
  | 'select'
  | 'switch'
  /** A device of the inventory, picked by name. */
  | 'device';

export interface FieldOption {
  value: string;
  label: string;
}

export interface Field {
  /** The parameter it feeds, which is also its key in {@link Values}. */
  name: string;
  label: string;
  kind: FieldKind;
  /** Always-visible help: the expected format, or a consequence. */
  hint?: string;
  placeholder?: string;
  options?: readonly FieldOption[];
  /** Shown only while another field has this value. */
  when?: { field: string; is: string };
  /** Columns of the form's two-column grid. */
  span?: 1 | 2;
  rows?: number;
}

/**
 * The form's state: every field as text, switches as `'on'` or `''`.
 *
 * One flat shape of strings, because it is what an `<input>` holds, what
 * `localStorage` can keep, and what a test can write by hand. Turning it into
 * typed parameters is `Spec.params`' job, in one place.
 */
export type Values = Record<string, string>;
