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
  /**
   * What choosing this option means, shown under the field while it is the
   * one chosen: a codec's bit rate, the bytes a link type adds.
   */
  hint?: string;
}

/**
 * A choice whose options are the engine's, not this screen's: the rows of one
 * of its reference tables (`tools.reference`). The table is the same one the
 * calculator reads its figures from, so a codec the engine gains is in the
 * picker without a line here, and one it does not have cannot be chosen.
 */
export interface FieldSource {
  /** The reference table's id: `voice_codecs`. */
  table: string;
  /** One row as an option. `null` leaves out a row that cannot be one. */
  option: (row: Record<string, string | number | boolean | null>) => FieldOption | null;
}

export interface Field {
  /** The parameter it feeds, which is also its key in {@link Values}. */
  name: string;
  label: string;
  kind: FieldKind;
  /** Always-visible help: the expected format, or a consequence. */
  hint?: string;
  placeholder?: string;
  /**
   * The choices of a `select`. With a {@link Field.source} these come after
   * the engine's rows: the choices the table has no row for.
   */
  options?: readonly FieldOption[];
  /** A `select` filled from a reference table instead of from `options` alone. */
  source?: FieldSource;
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
