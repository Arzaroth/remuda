import { DAY, MIN, usedTone, type Tone } from './format';
import type { Window } from './types';

export type Outlook = { text: string; tone: Tone; title: string };

const clock = (ms: number, now: number) => {
  const time = new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  if (new Date(ms).toDateString() === new Date(now).toDateString()) return time;
  if (ms - now >= 6 * DAY) return new Date(ms).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
  return `${new Date(ms).toLocaleDateString(undefined, { weekday: 'short' })} ${time}`;
};

// The server's forecast (`src/gauge.rs`) in words: when the window runs out
// at its recent burn, or where it ends.
export function outlook(w: Window, now: number): Outlook | null {
  const f = w.forecast;
  if (!f) return null;
  const rate = w.perHour != null ? `${w.perHour < 10 ? w.perHour.toFixed(1) : Math.round(w.perHour)}% an hour lately` : 'its recent burn';
  if ('fullAt' in f) {
    const at = Date.parse(f.fullAt);
    if (!Number.isFinite(at)) return null;
    return { text: `Full ${clock(at, now)} at this pace`, tone: 'bad', title: `At ${rate}` };
  }
  return { text: `Ends ~${f.ends}% at this pace`, tone: usedTone(f.ends), title: `At ${rate}` };
}

export type Trace = { line: string; ahead: string | null; now: number };

// The window's trail drawn across its length in a 100 x 100 box, y down: a
// figure holds until the next sample, and the window opens empty. `ahead`
// carries the forecast from now to where it lands.
export function trace(w: Window, now: number): Trace | null {
  if (!w.trail?.length || !w.minutes || !w.resetsAt) return null;
  const reset = Date.parse(w.resetsAt);
  const length = w.minutes * MIN;
  const opened = reset - length;
  if (!Number.isFinite(reset) || now <= opened || now >= reset) return null;
  const x = (at: number) => +(((Math.min(Math.max(at, opened), reset) - opened) / length) * 100).toFixed(2);
  const y = (used: number) => 100 - Math.min(Math.max(used, 0), 100);
  let held = 0;
  const points = [`M0 100`];
  for (const [at, used] of w.trail) {
    if (at > now) break;
    points.push(`H${x(at)}`, `V${y(used)}`);
    held = used;
  }
  const last = Math.max(held, w.usedPercent);
  points.push(`H${x(now)}`, `V${y(last)}`);
  const f = w.forecast;
  let ahead: string | null = null;
  if (f && 'fullAt' in f && Number.isFinite(Date.parse(f.fullAt))) ahead = `M${x(now)} ${y(last)} L${x(Date.parse(f.fullAt))} 0`;
  else if (f && 'ends' in f) ahead = `M${x(now)} ${y(last)} L100 ${y(f.ends)}`;
  return { line: points.join(' '), ahead, now: x(now) };
}
