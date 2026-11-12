import { describe, expect, it } from 'vitest';
import { arrange, change, changed, changeText, cutShort, FILTERS, kept, plans, rate, removed, weights, type Week } from './value';
import type { Login } from './spend';

const DAY = 86_400_000;
const week = (n: number, over: Partial<Week> = {}): Week => ({
  provider: 'codex',
  login: 'work',
  account: 'd-work',
  openedAt: n * 7 * DAY,
  endsAt: (n + 1) * 7 * DAY,
  open: false,
  used: 50,
  usd: 50,
  usdPerPercent: 1,
  unattributedUsd: 0,
  partial: false,
  unpriced: false,
  ...over,
});

describe('value', () => {
  it('groups the weeks by login and account, oldest first, newest login first', () => {
    const got = plans([week(1), week(3, { login: 'perso' }), week(2), week(0, { login: null }), week(4, { account: 'd-old' })]);
    expect(got.map((p) => p.login)).toEqual(['work', 'perso', 'work']);
    expect(got[0].weeks.map((w) => w.account)).toEqual(['d-old']);
    expect(got[2].weeks.map((w) => w.openedAt)).toEqual([7 * DAY, 14 * DAY]);
  });

  it('compares the last closed week with up to four before it', () => {
    const weeks = [0.5, 1, 1, 1, 1, 2].map((v, n) => week(n, { usdPerPercent: v }));
    expect(change([...weeks, week(6, { open: true, usdPerPercent: 9 })])).toBe(2);
    expect(change([week(0), week(1, { partial: true })])).toBeNull();
    expect(change([week(0), week(1, { unpriced: true, usdPerPercent: 0 })])).toBeNull();
    expect(change([week(0)])).toBeNull();
  });

  it('says how it moved', () => {
    expect(changeText(2)).toBe('2.0x the weeks before');
    expect(changeText(0.5)).toBe('50% under the weeks before');
    expect(changeText(1.02)).toBe('about the same as the weeks before');
  });

  it('prices a percent over the last four whole closed weeks with use', () => {
    const weeks = [
      week(0, { usd: 1000, used: 10 }),
      week(1, { usd: 100, used: 50 }),
      week(2, { usd: 300, used: 50 }),
      week(3, { usd: 50, used: 0 }),
      week(4, { usd: 999, used: 50, partial: true }),
      week(4, { usd: 999, used: 50, unattributedUsd: 20 }),
      week(5, { usd: 200, used: 100 }),
      week(6, { usd: 400, used: 100 }),
      week(7, { usd: 999, used: 20, open: true }),
    ];
    expect(rate(weeks)).toBeCloseTo((100 + 300 + 200 + 400) / 300);
    expect(rate([week(0, { open: true })])).toBeNull();
  });

  it('leaves out a week whose spend no login claims had no price', () => {
    expect(rate([week(0, { usd: 100, used: 50 }), week(1, { usd: 900, used: 50, unattributedUnpriced: true })])).toBeCloseTo(2);
  });

  it("measures each login's weight against the largest plan's dollars per percent", () => {
    const ps = plans([
      week(1, { login: 'max', usd: 200, used: 100 }),
      week(1, { login: 'pro', usd: 25, used: 100 }),
      week(1, { login: 'lone', provider: 'claude', usd: 25, used: 100 }),
    ]);
    const got = weights(ps, { codex: { max: 20, pro: 2 }, claude: { lone: 5 } });
    const of = (login: string) => got.get(ps.find((p) => p.login === login)!);
    expect(of('max')).toEqual({ advertised: 1, measured: 1 });
    expect(of('pro')!.advertised).toBeCloseTo(0.1);
    expect(of('pro')!.measured).toBeCloseTo(0.125);
    expect(of('lone')).toBeUndefined();
  });

  it('measures only against the largest plan', () => {
    const ps = plans([week(1, { login: 'pro', usd: 25, used: 100 }), week(1, { login: 'plus', usd: 10, used: 100 })]);
    expect(weights(ps, { codex: { max: 20, pro: 2, plus: 1 } }).size).toBe(0);
  });

  it('sets the measurement on the account it measured, not an older one under the same name', () => {
    const ps = plans([
      week(0, { login: 'max', account: 'old', usd: 900, used: 100 }),
      week(2, { login: 'max', account: 'new', usd: 200, used: 100 }),
      week(2, { login: 'pro', usd: 25, used: 100 }),
    ]);
    const got = weights(ps, { codex: { max: 20, pro: 2 } });
    expect(got.get(ps.find((p) => p.weeks[0].account === 'new')!)).toEqual({ advertised: 1, measured: 1 });
    expect(got.has(ps.find((p) => p.weeks[0].account === 'old')!)).toBe(false);
  });

  it('says after how many days a reset cut a week short', () => {
    expect(cutShort(week(0))).toBeNull();
    expect(cutShort(week(0, { endsAt: 3 * DAY }))).toBe(3);
    expect(cutShort(week(0, { endsAt: DAY, open: true }))).toBeNull();
    expect(cutShort(week(0, { endsAt: 7 * DAY - DAY / 48 }))).toBeNull();
  });

  it('keeps a week only when every filter lets it through', () => {
    const now = Date.UTC(2026, 10, 12);
    const at = (ms: number, over: Partial<Week> = {}) => week(0, { openedAt: ms, endsAt: ms + 7 * DAY, ...over });
    const f = (over: Partial<typeof FILTERS>) => ({ ...FILTERS, ...over });
    expect(kept(at(Date.UTC(2026, 9, 1)), f({ months: 1 }), now)).toBe(false);
    expect(kept(at(Date.UTC(2026, 9, 8)), f({ months: 1 }), now)).toBe(true);
    expect(kept(at(Date.UTC(2025, 9, 1)), f({}), now)).toBe(true);
    expect(kept(week(0, { used: 9 }), f({ minUsed: 10 }), now)).toBe(false);
    expect(kept(week(0, { used: 10 }), f({ minUsed: 10 }), now)).toBe(true);
    expect(kept(week(0, { unattributedUsd: 5 }), f({ unclaimed: true }), now)).toBe(false);
    expect(kept(week(0, { unattributedUsd: 0.001 }), f({ unclaimed: true }), now)).toBe(true);
    expect(kept(week(0, { open: true }), f({ open: true }), now)).toBe(false);
    expect(kept(week(0, { partial: true }), f({ incomplete: true }), now)).toBe(false);
    expect(kept(week(0, { unpriced: true }), f({ incomplete: true }), now)).toBe(false);
    expect(kept(week(0, { usd: 0 }), f({ idle: true }), now)).toBe(false);
    expect(kept(week(0, { endsAt: 2 * DAY }), f({ cut: true }), now)).toBe(false);
    expect(kept(week(0, { usd: 0, open: true, partial: true, endsAt: DAY }), FILTERS, now)).toBe(true);
  });

  it('counts the filters changed, the order aside', () => {
    expect(changed(FILTERS)).toBe(0);
    expect(changed({ ...FILTERS, order: 'name' })).toBe(0);
    expect(changed({ ...FILTERS, months: 3, removed: false })).toBe(2);
  });

  it('calls a login removed when the store dropped its account', () => {
    const logins: Record<string, Login> = {
      'd-work': { provider: 'codex', name: 'work' },
      'd-old': { provider: 'codex', name: 'old', removed: true },
    };
    const [work] = plans([week(0)]);
    const [old] = plans([week(0, { account: 'd-old' })]);
    const [lost] = plans([week(0, { account: 'd-lost' })]);
    const [unknown] = plans([week(0, { account: null })]);
    expect([work, old, lost, unknown].map((p) => removed(p, logins))).toEqual([false, true, true, false]);
    expect(removed(old, undefined)).toBe(false);
  });

  it('arranges the logins left after the filters and names those it left out', () => {
    const logins: Record<string, Login> = {
      'd-work': { provider: 'codex', name: 'work' },
      'd-perso': { provider: 'codex', name: 'perso' },
      'd-team': { provider: 'codex', name: 'team' },
      'd-old': { provider: 'codex', name: 'old', removed: true },
    };
    const ps = plans([
      week(5, { usd: 50, usdPerPercent: 1 }),
      week(4, { usd: 10, used: 5 }),
      week(4, { login: 'perso', account: 'd-perso', usd: 200, usdPerPercent: 4 }),
      week(3, { login: 'team', account: 'd-team', used: 2, usd: 1 }),
      week(2, { login: 'old', account: 'd-old' }),
    ]);
    const now = 100 * DAY;
    const got = arrange(ps, { ...FILTERS, minUsed: 10 }, logins, now);
    expect(got.shown.map((s) => [s.plan.login, s.weeks.length, s.hidden])).toEqual([
      ['work', 1, 1],
      ['perso', 1, 0],
    ]);
    expect(got.empty.map((p) => p.login)).toEqual(['team']);
    expect(got.gone.map((p) => p.login)).toEqual(['old']);
    expect(arrange(ps, { ...FILTERS, order: 'value' }, logins, now).shown.map((s) => s.plan.login)).toEqual(['perso', 'work', 'team']);
    expect(arrange(ps, { ...FILTERS, order: 'name' }, logins, now).shown.map((s) => s.plan.login)).toEqual(['perso', 'team', 'work']);
    expect(arrange(ps, { ...FILTERS, removed: false }, logins, now).gone).toEqual([]);
  });
});
