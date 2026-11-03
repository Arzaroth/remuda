import { describe, expect, it } from 'vitest';
import { change, changeText, plans, type Week } from './value';

const DAY = 86_400_000;
const week = (n: number, over: Partial<Week> = {}): Week => ({
  provider: 'codex',
  login: 'work',
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
  it('groups the weeks by login, oldest first, newest login first', () => {
    const got = plans([week(1), week(3, { login: 'perso' }), week(2), week(0, { login: null })]);
    expect(got.map((p) => p.login)).toEqual(['perso', 'work']);
    expect(got[1].weeks.map((w) => w.openedAt)).toEqual([7 * DAY, 14 * DAY]);
  });

  it('compares the last closed week with up to four before it', () => {
    const weeks = [0.5, 1, 1, 1, 1, 2].map((v, n) => week(n, { usdPerPercent: v }));
    expect(change([...weeks, week(6, { open: true, usdPerPercent: 9 })])).toBe(2);
    expect(change([week(0), week(1, { partial: true })])).toBeNull();
    expect(change([week(0)])).toBeNull();
  });

  it('says how it moved', () => {
    expect(changeText(2)).toBe('2.0x the weeks before');
    expect(changeText(0.5)).toBe('50% under the weeks before');
    expect(changeText(1.02)).toBe('about the same as the weeks before');
  });
});
