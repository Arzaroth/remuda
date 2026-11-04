import { describe, expect, it } from 'vitest';
import { HOUR, MIN } from './format';
import { burn, capacity, knownFrom, logins, pick, project, recorded, remaining, type Login } from './quota';
import type { Combined, Window } from './types';

const NOW = Date.UTC(2026, 10, 4, 12);
const WEEK = 7 * 24 * HOUR;

const login = (name: string, used: number, resetsIn: number | null, samples: [number, number][] = [], weight = 1): Login => ({
  name,
  weight,
  used,
  resetsAt: resetsIn === null ? null : NOW + resetsIn,
  length: WEEK,
  samples: [...samples, [NOW, used]],
});

const win = (used: number, resetsAt: string | null, trail: [number, number][] = [], minutes = 10080): Window => ({
  title: 'Weekly',
  minutes,
  usedPercent: used,
  resetsAt,
  trail,
});

const combined = (title: string, parts: [string, Window, number][]): Combined => ({
  title,
  weighted: true,
  used: 0,
  of: 0,
  pooled: 0,
  parts: parts.map(([name, window, weight]) => ({ name, active: false, window, weight })),
  widths: [],
  leftOut: [],
});

describe('logins', () => {
  it('weighs each login in units of the largest plan and ends its samples at now', () => {
    const ls = logins(
      combined('Weekly', [
        ['max', win(40, '2026-11-06T00:00:00Z', [[NOW - HOUR, 30], [NOW + HOUR, 99]]), 20],
        ['pro', win(10, null), 2],
      ]),
      NOW,
    );
    expect(ls.map((l) => l.weight)).toEqual([1, 0.1]);
    expect(ls[0].samples).toEqual([
      [NOW - HOUR, 30],
      [NOW, 40],
    ]);
    expect(ls[0].length).toBe(WEEK);
    expect(ls[1].resetsAt).toBeNull();
    expect(capacity(ls)).toBeCloseTo(110);
    expect(remaining(ls)).toBeCloseTo(69);
  });
});

describe('recorded', () => {
  it('starts once every login is known and marks a reset as a refill', () => {
    const ls = [
      login('a', 10, HOUR, [
        [NOW - 5 * HOUR, 20],
        [NOW - 3 * HOUR, 80],
        [NOW - 2 * HOUR, 5],
      ]),
      login('b', 50, HOUR, [[NOW - 4 * HOUR, 40]]),
    ];
    expect(knownFrom(ls)).toBe(NOW - 4 * HOUR);
    const r = recorded(ls);
    expect(r.points).toEqual([
      [NOW - 4 * HOUR, 140],
      [NOW - 3 * HOUR, 80],
      [NOW - 2 * HOUR, 155],
      [NOW, 140],
    ]);
    expect(r.refills).toEqual([{ at: NOW - 2 * HOUR, gain: 75, name: 'a' }]);
  });
});

describe('burn', () => {
  it('counts what the figures rose by over the hours asked, never a reset', () => {
    const ls = [
      login('a', 10, HOUR, [
        [NOW - 3 * HOUR, 20],
        [NOW - 2 * HOUR, 50],
        [NOW - HOUR, 4],
      ]),
      login('b', 30, HOUR, [[NOW - 3 * HOUR, 24]], 0.5),
    ];
    expect(burn(ls, NOW, 3)).toBeCloseTo((30 + 6 + 3) / 3);
    expect(burn(ls, NOW, 1)).toBeCloseTo(6 + 3);
  });

  it('measures only the time on record, and nothing under a quarter hour', () => {
    const ls = [login('a', 30, HOUR, [[NOW - 2 * HOUR, 10]])];
    expect(burn(ls, NOW, 6)).toBeCloseTo(10);
    expect(burn([login('a', 30, HOUR, [[NOW - 10 * MIN, 10]])], NOW, 1)).toBeNull();
    expect(burn([], NOW, 1)).toBeNull();
  });
});

describe('project', () => {
  it('drains the login that resets soonest, then refills it at its reset', () => {
    const ls = [login('late', 0, 10 * HOUR), login('soon', 50, 4 * HOUR)];
    const p = project(ls, NOW, 10, NOW + 6 * HOUR);
    expect(p.points).toEqual([
      [NOW, 150],
      [NOW + 4 * HOUR, 110],
      [NOW + 4 * HOUR, 200],
      [NOW + 6 * HOUR, 180],
    ]);
    expect(p.refills).toEqual([{ at: NOW + 4 * HOUR, gain: 90, name: 'soon' }]);
    expect(p.firstEmpty).toBeNull();
    expect(p.poolEmpty).toBeNull();
  });

  it('moves on to the next login when one runs dry and says when the pool empties', () => {
    const ls = [login('a', 80, 10 * HOUR), login('b', 90, 20 * HOUR)];
    const p = project(ls, NOW, 10, NOW + 12 * HOUR);
    expect(p.firstEmpty).toEqual({ at: NOW + 2 * HOUR, name: 'a' });
    expect(p.poolEmpty).toBe(NOW + 3 * HOUR);
    expect(p.points).toContainEqual([NOW + 3 * HOUR, 0]);
    expect(p.refills).toEqual([{ at: NOW + 10 * HOUR, gain: 100, name: 'a' }]);
    expect(p.points.at(-1)).toEqual([NOW + 12 * HOUR, 80]);
  });

  it('spends a lighter plan faster in its own percent', () => {
    const p = project([login('pro', 0, 100 * HOUR, [], 0.1)], NOW, 1, NOW + 20 * HOUR);
    expect(p.firstEmpty?.at).toBe(NOW + 10 * HOUR);
  });

  it('only refills at no burn, and a window starts again after its reset', () => {
    const p = project([login('a', 40, HOUR)], NOW, 0, NOW + WEEK + 2 * HOUR);
    expect(p.refills.map((r) => r.at)).toEqual([NOW + HOUR]);
    expect(p.points.at(-1)).toEqual([NOW + WEEK + 2 * HOUR, 100]);
  });
});

describe('pick', () => {
  it('takes the window picked, else the longest', () => {
    const five = combined('5 hours', [['a', win(1, null, [], 300), 1]]);
    const week = combined('Weekly', [['a', win(1, null), 1]]);
    expect(pick([five, week], undefined)).toBe(week);
    expect(pick([five, week], '5 hours')).toBe(five);
    expect(pick([five, week], 'gone')).toBe(week);
    expect(pick([], undefined)).toBeNull();
  });
});
