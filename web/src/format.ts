export const MIN = 60e3;
export const HOUR = 60 * MIN;
export const DAY = 24 * HOUR;

export function span(ms: number): string {
  const a = Math.abs(ms);
  if (a < MIN) return 'under a minute';
  const [n, unit] =
    a < HOUR ? [Math.floor(a / MIN), 'minute'] : a < DAY ? [Math.floor(a / HOUR), 'hour'] : [Math.floor(a / DAY), 'day'];
  return `${n} ${unit}${n === 1 ? '' : 's'}`;
}

export const until = (ms: number, now = Date.now()) =>
  ms < now ? `${span(ms - now)} ago` : `in ${span(ms - now)}`;
export const ago = (ms: number, now = Date.now()) => `${span(now - ms)} ago`;
export const date = (ms: number) =>
  new Date(ms).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });

export type Tone = '' | 'warn' | 'bad';
export const usedTone = (p: number): Tone => (p >= 90 ? 'bad' : p >= 70 ? 'warn' : '');
export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;
