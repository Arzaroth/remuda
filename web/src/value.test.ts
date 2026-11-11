import { describe, expect, it } from 'vitest';
import { change, changeText, plans, rate, weights, type Week } from './value';

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
});
