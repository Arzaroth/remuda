import { describe, expect, it } from 'vitest';
import { by, cacheHit, compact, daily, dates, loginName, money, since, sum, total, type SpendRow } from './spend';

const row = (over: Partial<SpendRow>): SpendRow => ({
  date: '2026-10-30',
  provider: 'claude',
  model: 'claude-x',
  login: null,
  source: 'cli',
  tokens: { i: 10, o: 20, cw5: 3, cw1h: 2, cr: 65 },
  usd: { input: 1, output: 2, cacheWrite: 0.5, cacheRead: 0.25 },
  ...over,
});

describe('spend', () => {
  it('sums cost and tokens by rate, and says when a row had no price', () => {
    const s = sum([row({ sessions: 2 }), row({ usd: null, model: 'unknown' })]);
    expect(s.usd).toBe(3.75);
    expect(s.unpriced).toBe(true);
    expect(s.tokens).toEqual({ input: 20, output: 40, cacheWrite: 10, cacheRead: 130 });
    expect(total(s.tokens)).toBe(200);
    expect(s.sessions).toBe(2);
    expect(sum([row({})]).unpriced).toBe(false);
  });

  it('groups by any key, the largest spend first', () => {
    const groups = by([row({ model: 'a' }), row({ model: 'b', usd: { input: 9, output: 0, cacheWrite: 0, cacheRead: 0 } })], (r) => r.model);
    expect(groups.map(([k]) => k)).toEqual(['b', 'a']);
  });

  it('spreads rows over every day of the range, empty days included', () => {
    const days = dates('2026-10-29', '2026-10-31');
    expect(days).toEqual(['2026-10-29', '2026-10-30', '2026-10-31']);
    const per = daily([row({}), row({ date: '2026-09-01' })], days, (r) => r.provider, () => 1);
    expect(per.get('claude')).toEqual([0, 1, 0]);
  });

  it('starts a range so it ends today', () => {
    expect(since(7, new Date('2026-11-01T09:00:00'))).toBe('2026-10-26');
    expect(since(1, new Date('2026-11-01T23:00:00'))).toBe('2026-11-01');
  });

  it('reads the cache hit off the prompt tokens', () => {
    expect(cacheHit({ input: 10, output: 999, cacheWrite: 10, cacheRead: 80 })).toBe(0.8);
    expect(cacheHit({ input: 0, output: 5, cacheWrite: 0, cacheRead: 0 })).toBeNull();
  });

  it('names a login, or says why it cannot', () => {
    const logins = { aa: { provider: 'claude', name: 'work', label: 'Axeo' }, bb: { provider: 'claude', name: 'perso' } };
    expect(loginName('aa', logins)).toBe('Axeo (work)');
    expect(loginName('bb', logins)).toBe('perso');
    expect(loginName(null, logins)).toBe('Unattributed');
    expect(loginName('cc', logins)).toBe('A login no longer stored');
    expect(loginName('dd', { dd: { provider: 'claude', name: 'old', removed: true } })).toBe('old (removed)');
  });

  it('writes large counts short and small sums with a third digit', () => {
    expect(compact(75_800_000_000)).toBe('75.8B');
    expect(compact(1500)).toBe('1.5K');
    expect(compact(999)).toBe('999');
    expect(money(0.25)).toContain('0.250');
  });
});
