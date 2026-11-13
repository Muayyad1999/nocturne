/**
 * Maintenance windows are the one place in this screen where a person types a
 * time rather than reading one. `<input type="datetime-local">` speaks
 * "YYYY-MM-DDTHH:mm" in the browser's own time zone; the backend speaks
 * milliseconds since the epoch. These two turn one into the other without
 * going through a string the user did not write.
 */

function pad(value: number): string {
  return String(value).padStart(2, '0');
}

/** A timestamp as `datetime-local` wants it, in local time. */
export function toLocalInput(at: number): string {
  const when = new Date(at);
  return `${when.getFullYear()}-${pad(when.getMonth() + 1)}-${pad(when.getDate())}T${pad(when.getHours())}:${pad(when.getMinutes())}`;
}

/** What `datetime-local` gives back, as a timestamp. `null` when it is not a time. */
export function fromLocalInput(text: string): number | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(text.trim());
  if (!match) return null;
  const [, year, month, day, hour, minute, second] = match;
  const at = new Date(
    Number(year),
    Number(month) - 1,
    Number(day),
    Number(hour),
    Number(minute),
    Number(second ?? 0),
    0,
  );
  return Number.isNaN(at.getTime()) ? null : at.getTime();
}

/** Where a window stands right now, for its row. */
export type WindowState = 'open' | 'upcoming' | 'past';

export function windowState(
  window: { starts_at: number; ends_at: number; recurring: string | null },
  now: number = Date.now(),
): WindowState {
  if (now >= window.starts_at && now < window.ends_at) return 'open';
  if (now < window.starts_at) return 'upcoming';
  // A weekly window that has passed this week comes round again next week.
  return window.recurring === 'weekly' ? 'upcoming' : 'past';
}
