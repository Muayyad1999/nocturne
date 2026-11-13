/**
 * Tones name what a colour means. Status tones are reserved for state and are
 * always paired with a shape or a word; `accent` is the brand, `ai` marks
 * anything the assistant did or proposes, `neutral` carries no meaning.
 */

export type StatusTone = 'ok' | 'warn' | 'crit' | 'info' | 'idle' | 'maint';

export type Tone = 'neutral' | 'accent' | 'ai' | StatusTone;

/** The word that goes with each status when a screen has nothing more specific. */
export const STATUS_LABEL: Record<StatusTone, string> = {
  ok: 'Healthy',
  warn: 'Degraded',
  crit: 'Critical',
  info: 'Info',
  idle: 'Unknown',
  maint: 'Maintenance',
};
