import type { LogEvent } from './types';

export type LogFilter = { provider: string | null; failures: boolean };

export type LogDay = { key: string; at: number; events: LogEvent[] };

const dayKey = (ms: number) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;
};

/** The events the filter keeps, newest first, in one group per local day. */
export function days(events: LogEvent[], filter: LogFilter): LogDay[] {
  const out: LogDay[] = [];
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (filter.provider && e.provider !== filter.provider) continue;
    if (filter.failures && e.ok) continue;
    const key = dayKey(e.at);
    const last = out[out.length - 1];
    if (last?.key === key) last.events.push(e);
    else out.push({ key, at: e.at, events: [e] });
  }
  return out;
}

export const subject = (e: LogEvent) => (e.name ? `${e.provider}/${e.name}` : (e.provider ?? ''));

export const byLabel: Record<NonNullable<LogEvent['by']>, string> = {
  cli: 'from the CLI',
  page: 'from the page',
  auto: 'by auto mode',
};
