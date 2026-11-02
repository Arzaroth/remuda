import { describe, expect, it } from 'vitest';
import { averageLatency, successRate, tally, tallyBy, tps, type RequestRow } from './requests';

const row = (over: Partial<RequestRow>): RequestRow => ({
  at: Date.parse('2026-10-30T10:00:00'),
  provider: 'claude',
  model: 'claude-x',
  login: 'work',
  key: 'laptop',
  outcome: 'ok',
  requests: 2,
  tokens: { i: 10, o: 40, cr: 50 },
  reasoning: 0,
  latencyMs: 3000,
  generatingMs: 2000,
  generated: 40,
  ...over,
});

describe('requests', () => {
  it('counts outcomes and leaves canceled ones out of the success rate', () => {
    const t = tally([row({}), row({ outcome: 'failed', requests: 2 }), row({ outcome: 'canceled', requests: 6 })]);
    expect([t.requests, t.ok, t.failed, t.canceled]).toEqual([10, 2, 2, 6]);
    expect(successRate(t)).toBe(0.5);
  });

  it('measures speed while writing and latency per request', () => {
    const t = tally([row({}), row({ generatingMs: 2000, generated: 160 })]);
    expect(tps(t)).toBe(50);
    expect(averageLatency(t)).toBe(1500);
    expect(tps(tally([row({ generatingMs: 0, generated: 0 })]))).toBeNull();
  });

  it('groups by model, the most tokens first', () => {
    const groups = tallyBy([row({ model: 'small' }), row({ model: 'big', tokens: { o: 1000 } })], (r) => r.model);
    expect(groups.map(([m]) => m)).toEqual(['big', 'small']);
  });
});
