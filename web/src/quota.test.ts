import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { HOUR, MIN } from './format';
import { burn, capacity, knownFrom, logins, pick, project, rateAt, recorded, remaining, shares, usual, type Login, type Point } from './quota';
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
  past: [],
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

  it('keeps no sample newer than the snapshot it ends on, and holds the snapshot from its time', () => {
    const ls = logins(combined('Weekly', [['max', win(40, null, [[NOW - 2 * HOUR, 30], [NOW - HOUR, 45]]), 1]]), NOW, NOW - 90 * MIN);
    expect(ls[0].samples).toEqual([
      [NOW - 2 * HOUR, 30],
      [NOW - 90 * MIN, 40],
      [NOW, 40],
    ]);
    expect(burn(ls, NOW, 1)).toBeCloseTo(0);
  });

  it('carries the closed instances TokenGauge kept the samples of', () => {
    const past = [{ openedAt: NOW - 2 * WEEK, samples: [[NOW - WEEK - HOUR, 80]] as [number, number][] }];
    const ls = logins(combined('Weekly', [['max', { ...win(40, null), past }, 1]]), NOW);
    expect(ls[0].past).toEqual([{ opened: NOW - 2 * WEEK, samples: [[NOW - WEEK - HOUR, 80]] }]);
  });

  it('never backdates a figure that has no trail, such as a window reset since the snapshot', () => {
    const ls = logins(combined('Weekly', [['max', { ...win(0, null), trail: null }, 1]]), NOW, NOW - 90 * MIN);
    expect(ls[0].samples).toEqual([[NOW, 0]]);
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
  it('counts what the figures rose by over the hours asked, never a reset, a rise sampled across the start in part', () => {
    const ls = [
      login('a', 10, HOUR, [
        [NOW - 3 * HOUR, 20],
        [NOW - 2 * HOUR, 50],
        [NOW - HOUR, 4],
      ]),
      login('b', 30, HOUR, [[NOW - 3 * HOUR, 24]], 0.5),
    ];
    expect(burn(ls, NOW, 3)).toBeCloseTo((30 + 6 + 3) / 3);
    expect(burn(ls, NOW, 1)).toBeCloseTo(6 + 1);
  });

  it('measures only the time on record, and nothing under a quarter hour', () => {
    const ls = [login('a', 30, HOUR, [[NOW - 2 * HOUR, 10]])];
    expect(burn(ls, NOW, 6)).toBeCloseTo(10);
    expect(burn([login('a', 30, HOUR, [[NOW - 10 * MIN, 10]])], NOW, 1)).toBeNull();
    expect(burn([], NOW, 1)).toBeNull();
  });
});

describe('shares', () => {
  it("gives each login its own burn and its part of the pool's, which add up to the pool's", () => {
    const ls = [login('max', 40, 20 * HOUR, [[NOW - 2 * HOUR, 30]]), login('pro', 70, 20 * HOUR, [[NOW - 2 * HOUR, 50]], 0.25)];
    const each = shares(ls, NOW, 3);
    expect(each.map((s) => [s.name, s.left, s.own, s.pooled])).toEqual([
      ['max', 60, 5, 5],
      ['pro', 30, 10, 2.5],
    ]);
    expect(each.reduce((a, s) => a + s.pooled!, 0)).toBeCloseTo(burn(ls, NOW, 3)!);
  });

  it('says when a login runs dry at its own pace, unless its reset comes first or it is not burning', () => {
    const ls = [
      login('dry', 40, 20 * HOUR, [[NOW - 2 * HOUR, 30]]),
      login('saved', 40, 4 * HOUR, [[NOW - 2 * HOUR, 30]]),
      login('idle', 40, null, [[NOW - 2 * HOUR, 40]]),
    ];
    expect(shares(ls, NOW, 3).map((s) => s.empty)).toEqual([NOW + 12 * HOUR, null, null]);
    expect(shares(ls, NOW, 3)[2].own).toBe(0);
  });

  it('has no burn while too little is on record', () => {
    const [s] = shares([login('a', 30, HOUR, [[NOW - 10 * MIN, 10]])], NOW, 1);
    expect([s.own, s.pooled, s.empty, s.left]).toEqual([null, null, null, 70]);
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

  it('opens an unopened window at its first spend', () => {
    const p = project([login('idle', 0, null)], NOW, 10, NOW + WEEK + HOUR);
    expect(p.firstEmpty).toEqual({ at: NOW + 10 * HOUR, name: 'idle' });
    expect(p.refills).toEqual([{ at: NOW + WEEK, gain: 100, name: 'idle' }]);
  });

  it('refills a reset already past at once, never stepping back', () => {
    const p = project([login('a', 70, -HOUR)], NOW, 0, NOW + HOUR);
    expect(p.refills).toEqual([{ at: NOW, gain: 70, name: 'a' }]);
    expect(p.points.every(([t], i) => !i || t >= p.points[i - 1][0])).toBe(true);
    expect(p.points.at(-1)).toEqual([NOW + HOUR, 100]);
  });

  it('says an empty pool is empty now', () => {
    expect(project([login('a', 100, HOUR)], NOW, 10, NOW + 2 * HOUR).poolEmpty).toBe(NOW);
  });

  it('only refills at no burn, and a window starts again after its reset', () => {
    const p = project([login('a', 40, HOUR)], NOW, 0, NOW + WEEK + 2 * HOUR);
    expect(p.refills.map((r) => r.at)).toEqual([NOW + HOUR]);
    expect(p.points.at(-1)).toEqual([NOW + WEEK + 2 * HOUR, 100]);
  });
});

const at = (day: number, hour: number, minute = 0) => new Date(2026, 10, 11 + day, hour, minute).getTime();
const MID = at(0, 0);

function mornings(from: number, to: number, start = 0): Point[] {
  const out: Point[] = [];
  let used = start;
  for (let d = from; d < to; d++) {
    out.push([at(d, 9), used]);
    used = Math.min(100, used + 9);
    out.push([at(d, 12), used]);
  }
  return out;
}

const week = (name: string, used: number, past: Point[], weight = 1): Login => ({
  name,
  weight,
  used,
  resetsAt: at(7, 0),
  length: WEEK,
  samples: [[MID, used]],
  past: [{ opened: at(-7, 0), samples: past }],
});

describe('usual', () => {
  it("learns which hours of the week spend the pool, and what a week of them needs", () => {
    const u = usual([week('max', 0, mornings(-7, 0))], MID)!;
    expect(u.days).toBeCloseTo(6.5);
    expect(u.week).toBeCloseTo(63);
    expect(rateAt(u.slots)(at(1, 10))).toBeCloseTo(3);
    expect(rateAt(u.slots)(at(1, 15))).toBeCloseTo(0);
    expect(rateAt(u.slots)(at(1, 3))).toBeCloseTo(0);
  });

  it('weighs each login and counts only the hours every login is on record', () => {
    const big = week('max', 0, mornings(-7, 0));
    const small = { ...week('pro', 0, mornings(-3, 0)), weight: 0.5, past: [{ opened: at(-3, 0), samples: mornings(-3, 0) }] };
    const u = usual([big, small], MID)!;
    expect(u.days).toBeCloseTo(2.5);
    expect(rateAt(u.slots)(at(1, 10))).toBeCloseTo(4.5);
  });

  it('leaves out the hours every login sat full, since nothing could be spent in them', () => {
    const capped = [...mornings(-7, -3), [at(-3, 12), 100], [at(-1, 12), 100]] as Point[];
    const u = usual([week('max', 0, capped)], MID)!;
    expect(u.days).toBeCloseTo(4.5);
  });

  it('starts a week refilled in place where the instance before it was last seen', () => {
    const l: Login = {
      ...week('max', 9, []),
      resetsAt: at(1, 0),
      samples: [
        [at(-3, 12), 9],
        [MID, 9],
      ],
      past: [{ opened: at(-7, 0), samples: mornings(-7, -3) }],
    };
    const u = usual([l], MID)!;
    expect(u.days).toBeCloseTo(7);
    expect(rateAt(u.slots)(at(-5, 3))).toBeCloseTo(0);
    expect(rateAt(u.slots)(at(-4, 18))).toBeCloseTo(9 / 24);
  });

  it('takes no hours from a window with no trail, whatever its opening', () => {
    const l: Login = { ...week('max', 40, []), resetsAt: at(1, 0), past: [] };
    expect(usual([l], MID)).toBeNull();
  });

  it('has no usual week with under a day on record', () => {
    const l = { ...week('max', 0, []), past: [{ opened: at(-1, 0), samples: mornings(-1, 0) }] };
    expect(usual([l], MID)).toBeNull();
  });

  it('gives each login its own usual burn and when it runs dry in a usual week', () => {
    const l = week('max', 95, mornings(-7, 0));
    const u = usual([l], MID)!;
    const [share] = shares([l], MID, 3, u);
    expect(share.own).toBeCloseTo(63 / 168);
    expect(share.empty).toBeCloseTo(at(0, 10, 40), -3);
  });
});

describe('usual, half an hour off UTC', () => {
  const zone = process.env.TZ;
  beforeAll(() => {
    process.env.TZ = 'Asia/Kolkata';
  });
  afterAll(() => {
    process.env.TZ = zone;
  });

  it("keeps a morning's hours on the clock's hours", () => {
    const u = usual([week('max', 0, mornings(-7, 0))], at(0, 0))!;
    expect(rateAt(u.slots)(at(1, 9, 15))).toBeCloseTo(3);
    expect(rateAt(u.slots)(at(1, 8, 45))).toBeCloseTo(0);
    expect(rateAt(u.slots)(at(1, 12, 15))).toBeCloseTo(0);
  });
});

describe('project at an hourly pace', () => {
  it('holds the pool through hours that usually spend nothing', () => {
    const ls = [week('max', 0, [])];
    const pace = (t: number) => (new Date(t).getHours() >= 9 && new Date(t).getHours() < 12 ? 3 : 0);
    const { points } = project(ls, MID, pace, at(1, 0));
    const value = (t: number) => points.find(([p]) => p === t)?.[1];
    expect(value(at(0, 9))).toBeCloseTo(100);
    expect(value(at(0, 12))).toBeCloseTo(91);
    expect(value(at(0, 23))).toBeCloseTo(91);
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
