/**
 * The pieces every settings section is built from: one setting per row with
 * its help, a group heading, and the hook that reads and writes a settings
 * section with a quiet confirmation.
 *
 * These live here rather than in `design/` because only Settings needs them.
 */

import { Check } from 'lucide-react';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { SettingsSetParams } from '@/api/types/core';
import { Icon } from '@/design/Icon';
import styles from './Settings.module.css';

export interface SettingProps {
  label: string;
  /** One sentence: what this does, or what it will cost you. */
  help?: ReactNode;
  /** Shown instead of the help when the value was refused. */
  error?: ReactNode;
  /** The control. Give it `aria-label={label}`: the row's label is text. */
  children: ReactNode;
  /** Briefly confirms that the value reached the backend. */
  saved?: boolean;
  /** Pulls the control to its natural width instead of filling the row. */
  narrow?: boolean;
}

/** One setting: its name, its control, and one sentence of help. */
export function Setting({ label, help, error, children, saved, narrow }: SettingProps) {
  return (
    <div className={styles.setting} data-narrow={narrow || undefined}>
      <div className={styles.settingText}>
        <span className={styles.settingLabel}>{label}</span>
        {(error || help) && (
          <p className={styles.settingHelp} data-invalid={Boolean(error) || undefined}>
            {error ?? help}
          </p>
        )}
      </div>
      <div className={styles.settingControl}>
        {children}
        <span className={styles.saved} data-show={saved || undefined} aria-live="polite">
          {saved && (
            <>
              <Icon icon={Check} size={12} /> Saved
            </>
          )}
        </span>
      </div>
    </div>
  );
}

export interface GroupProps {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
}

/** A heading over a run of settings. */
export function Group({ title, description, actions, children }: GroupProps) {
  return (
    <section className={styles.group}>
      <header className={styles.groupHead}>
        <div>
          <h3 className={styles.groupTitle}>{title}</h3>
          {description && <p className={styles.groupText}>{description}</p>}
        </div>
        {actions && <div className={styles.groupActions}>{actions}</div>}
      </header>
      <div className={styles.groupBody}>{children}</div>
    </section>
  );
}

/** How long the "Saved" tick stays on screen. */
const CONFIRM_MS = 1800;

export interface SettingsSectionState<T> {
  value: T | undefined;
  loading: boolean;
  error: unknown;
  refetch: () => void;
  /** Writes the given keys; everything else in the section is left alone. */
  set: (patch: Partial<T>) => void;
  /** The key that was just saved, for its row's confirmation. */
  justSaved: string | null;
  saveError: { key: string; message: string } | null;
}

/**
 * Reads a settings section and writes parts of it back. `settings.set` merges,
 * so one changed key never overwrites the rest of the section.
 */
export function useSettingsSection<T extends object>(section: string): SettingsSectionState<T> {
  const query = useRpc<T>('settings.get', { section }, { invalidateOn: ['settings.changed'] });
  const save = useRpcMutation<T, SettingsSetParams>('settings.set', {
    invalidates: ['settings.'],
    errorToast: false,
  });

  const [justSaved, setJustSaved] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<{ key: string; message: string } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  return {
    value: query.data,
    loading: query.isPending,
    error: query.error,
    refetch: () => void query.refetch(),
    justSaved,
    saveError,
    set: (patch) => {
      const key = Object.keys(patch)[0] ?? section;
      setSaveError(null);
      save.mutate(
        { section, value: patch as Record<string, unknown> },
        {
          onSuccess: () => {
            setJustSaved(key);
            clearTimeout(timer.current);
            timer.current = setTimeout(() => setJustSaved(null), CONFIRM_MS);
          },
          onError: (error) => setSaveError({ key, message: error.message }),
        },
      );
    },
  };
}
