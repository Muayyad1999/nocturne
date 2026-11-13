/**
 * General — how Nocturne looks and how it writes times.
 *
 * The preference is stored by the engine (`settings.general`), so it follows
 * the user's data rather than the browser. Theme is also applied here and
 * now, because a setting you cannot see take effect is not a setting.
 */

import type { GeneralSettings } from '@/api/types/core';
import { ErrorState } from '@/design/ErrorState';
import { RadioGroup } from '@/design/RadioGroup';
import { Skeleton } from '@/design/Skeleton';
import { Switch } from '@/design/Switch';
import { useTheme } from '@/design/theme';
import { formatTimestamp } from '@/lib/time';
import { Group, Setting, useSettingsSection } from './parts';
import styles from './Settings.module.css';

const THEMES = [
  { value: 'night', label: 'Night', description: 'The default: a dark sky, for a dark room.' },
  { value: 'day', label: 'Day', description: 'Light surfaces, for a bright one.' },
  { value: 'system', label: 'Match the system', description: 'Follow this computer’s own setting.' },
];

const DENSITIES = [
  { value: 'compact', label: 'Compact', description: 'More rows on screen.' },
  { value: 'comfortable', label: 'Comfortable', description: 'More room around each row.' },
];

/** `night` / `day` / `system` as the two themes the interface actually has. */
function resolveTheme(choice: string): 'dark' | 'light' {
  if (choice === 'day') return 'light';
  if (choice === 'night') return 'dark';
  return window.matchMedia?.('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}

export function General() {
  const section = useSettingsSection<GeneralSettings>('general');
  const setTheme = useTheme((state) => state.setTheme);
  const value = section.value;

  if (section.error) {
    return (
      <ErrorState
        title="These settings could not be read"
        error={section.error}
        onRetry={section.refetch}
      />
    );
  }
  if (!value) return <Skeleton height={220} shape="block" />;

  const now = Date.now();

  return (
    <div className={styles.stack}>
      <Group title="Appearance" description="Kept with your data, so it follows you to another machine.">
        <Setting
          label="Theme"
          help="Night is the default: Nocturne is made to be read at three in the morning. The moon in the header switches theme for this browser alone; this is the preference that is kept with your data."
          saved={section.justSaved === 'theme'}
          error={section.saveError?.key === 'theme' ? section.saveError.message : undefined}
        >
          <RadioGroup
              aria-label="Theme"
              options={THEMES}
              value={value.theme}
              onChange={(theme) => {
                setTheme(resolveTheme(theme));
                section.set({ theme });
              }}
            />
        </Setting>

        <Setting
          label="Density"
          help="How tightly tables and lists are packed."
          saved={section.justSaved === 'density'}
          error={section.saveError?.key === 'density' ? section.saveError.message : undefined}
        >
          <RadioGroup
            aria-label="Density"
            options={DENSITIES}
            value={value.density}
            onChange={(density) => section.set({ density })}
          />
        </Setting>
      </Group>

      <Group title="Time">
        <Setting
          label="Clock"
          help={`Times are written in this computer’s own zone. Right now: ${formatTimestamp(now)}.`}
          saved={section.justSaved === 'time_format'}
          error={section.saveError?.key === 'time_format' ? section.saveError.message : undefined}
        >
          <RadioGroup
            aria-label="Clock"
            orientation="horizontal"
            options={[
              { value: '24h', label: '24-hour' },
              { value: '12h', label: '12-hour' },
            ]}
            value={value.time_format}
            onChange={(time_format) => section.set({ time_format })}
          />
        </Setting>
      </Group>

      <Group
        title="Desktop app"
        description="These apply to the desktop build; in a browser they have no effect."
      >
        <Setting
          narrow
          label="Keep running in the tray"
          help="Closing the window hides it instead of quitting, so monitoring, receivers and schedules keep running."
          saved={section.justSaved === 'keep_running_in_tray'}
          error={
            section.saveError?.key === 'keep_running_in_tray' ? section.saveError.message : undefined
          }
        >
          <Switch
            aria-label="Keep running in the tray"
            checked={value.keep_running_in_tray}
            onChange={(event) => section.set({ keep_running_in_tray: event.target.checked })}
          />
        </Setting>
      </Group>
    </div>
  );
}
