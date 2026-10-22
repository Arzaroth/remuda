import { describe, expect, it } from 'vitest';
import { days, subject } from './log';
import type { LogEvent } from './types';

const at = (iso: string) => new Date(iso).getTime();
const ev = (when: string, provider: string, ok = true, name?: string): LogEvent => ({
  at: at(when),
  kind: 'switch',
  provider,
  name,
  ok,
  message: `${provider} ${when}`,
});

describe('the log', () => {
  const events = [
    ev('2026-10-21T09:00', 'claude', true, 'work'),
    ev('2026-10-21T18:00', 'codex', false),
    ev('2026-10-22T08:00', 'claude', false, 'perso'),
    ev('2026-10-22T09:00', 'codex'),
  ];

  it('shows the newest first, a group per day', () => {
    const shown = days(events, { provider: null, failures: false });
    expect(shown.map((d) => d.events.map((e) => e.message))).toEqual([
      ['codex 2026-10-22T09:00', 'claude 2026-10-22T08:00'],
      ['codex 2026-10-21T18:00', 'claude 2026-10-21T09:00'],
    ]);
  });

  it('narrows to a CLI and to failures', () => {
    expect(days(events, { provider: 'claude', failures: false }).flatMap((d) => d.events)).toHaveLength(2);
    const failed = days(events, { provider: null, failures: true }).flatMap((d) => d.events.map(subject));
    expect(failed).toEqual(['claude/perso', 'codex']);
    expect(days(events, { provider: 'grok', failures: false })).toEqual([]);
  });
});
