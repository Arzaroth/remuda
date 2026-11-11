import { HOUR, MIN } from './format';
import type { Combined } from './types';

export type Login = {
  name: string;
  weight: number;
  used: number;
  resetsAt: number | null;
  length: number | null;
  samples: [number, number][];
  past: Instance[];
};

export type Instance = { opened: number | null; samples: Point[] };

export type Point = [number, number];
export type Refill = { at: number; gain: number; name: string };

export function logins(c: Combined, now: number, written: number | null = null): Login[] {
  const top = Math.max(...c.parts.map((p) => p.weight), 0) || 1;
  const until = Math.min(now, written ?? now);
  return c.parts.map((p) => {
    const at = p.window.resetsAt ? Date.parse(p.window.resetsAt) : NaN;
    const trail = (p.window.trail ?? []).filter(([t]) => t <= until);
    return {
      name: p.name,
      weight: p.weight / top,
      used: p.window.usedPercent,
      resetsAt: Number.isFinite(at) ? at : null,
      length: p.window.minutes ? p.window.minutes * MIN : null,
      samples: [...trail, ...(p.window.trail && until < now ? [[until, p.window.usedPercent] as Point] : []), [now, p.window.usedPercent]],
      past: (p.window.past ?? []).map((i) => ({ opened: i.openedAt, samples: i.samples })),
    };
  });
}

export const capacity = (ls: Login[]) => ls.reduce((a, l) => a + l.weight * 100, 0);

export const remaining = (ls: Login[]) => ls.reduce((a, l) => a + l.weight * (100 - l.used), 0);

export const knownFrom = (ls: Login[]) => Math.max(...ls.map((l) => l.samples[0][0]));

function usedAt(l: Login, t: number): number {
  let [was, used] = l.samples[0];
  for (const [at, u] of l.samples) {
    if (at > t) return u > used && at > was ? used + ((u - used) * Math.max(0, t - was)) / (at - was) : used;
    [was, used] = [at, u];
  }
  return used;
}

export function recorded(ls: Login[]): { points: Point[]; refills: Refill[] } {
  if (!ls.length) return { points: [], refills: [] };
  const start = knownFrom(ls);
  const times = [...new Set(ls.flatMap((l) => l.samples.map(([t]) => t)).filter((t) => t >= start))].sort((a, b) => a - b);
  const refills: Refill[] = [];
  const seen = ls.map(() => 0);
  const used = ls.map((l) => l.samples[0][1]);
  const points = times.map((t): Point => {
    ls.forEach((l, i) => {
      while (seen[i] + 1 < l.samples.length && l.samples[seen[i] + 1][0] <= t) {
        const [at, u] = l.samples[++seen[i]];
        if (at > start && u < used[i]) refills.push({ at, gain: l.weight * (used[i] - u), name: l.name });
        used[i] = u;
      }
    });
    return [t, ls.reduce((a, l, i) => a + l.weight * (100 - used[i]), 0)];
  });
  return { points, refills };
}

function since(ls: Login[], at: number, hours: number): number | null {
  if (!ls.length) return null;
  const from = Math.max(at - hours * HOUR, knownFrom(ls));
  return at - from < 15 * MIN ? null : from;
}

function rise(l: Login, from: number, at: number): number {
  let spent = 0;
  let held = usedAt(l, from);
  for (const [t, u] of l.samples) {
    if (t <= from) continue;
    if (t > at) break;
    if (u > held) spent += u - held;
    held = u;
  }
  return spent;
}

export function burn(ls: Login[], at: number, hours: number): number | null {
  const from = since(ls, at, hours);
  if (from === null) return null;
  return ls.reduce((a, l) => a + l.weight * rise(l, from, at), 0) / ((at - from) / HOUR);
}

export type Share = {
  name: string;
  weight: number;
  left: number;
  resetsAt: number | null;
  own: number | null;
  pooled: number | null;
  empty: number | null;
};

export function shares(ls: Login[], at: number, hours: number, week: Usual | null = null): Share[] {
  const from = since(ls, at, hours);
  return ls.map((l, i) => {
    const slots = week?.own[i] ?? null;
    const own = week ? (slots ? slots.reduce((a, v) => a + v, 0) / slots.length : null) : from === null ? null : rise(l, from, at) / ((at - from) / HOUR);
    const dry = slots ? emptyBy(l, at, rateAt(slots)) : own ? at + ((100 - l.used) / own) * HOUR : null;
    return {
      name: l.name,
      weight: l.weight,
      left: 100 - l.used,
      resetsAt: l.resetsAt,
      own,
      pooled: own === null ? null : own * l.weight,
      empty: dry !== null && (l.resetsAt === null || dry < l.resetsAt) ? dry : null,
    };
  });
}

export type Projection = {
  points: Point[];
  refills: Refill[];
  firstEmpty: { at: number; name: string } | null;
  poolEmpty: number | null;
};

export type Rate = number | ((t: number) => number);

export function project(ls: Login[], now: number, pace: Rate, until: number): Projection {
  const rateOf = typeof pace === 'number' ? () => pace : pace;
  const left = ls.map((l) => 100 - l.used);
  const next = ls.map((l) => l.resetsAt);
  const pool = () => ls.reduce((a, l, i) => a + l.weight * left[i], 0);
  const points: Point[] = [[now, pool()]];
  const refills: Refill[] = [];
  let firstEmpty: Projection['firstEmpty'] = null;
  let poolEmpty: number | null = ls.length && pool() === 0 ? now : null;
  let t = now;
  for (let guard = 0; t < until && guard < 10_000; guard++) {
    const rate = rateOf(t);
    const open = ls.map((_, i) => i).filter((i) => left[i] > 0);
    const d = open.length ? open.reduce((a, i) => ((next[i] ?? Infinity) < (next[a] ?? Infinity) ? i : a)) : null;
    if (d !== null && rate > 0 && next[d] === null && ls[d].length) next[d] = t + ls[d].length;
    const soonest = Math.max(t, Math.min(...next.map((n) => n ?? Infinity)));
    const dry = d !== null && rate > 0 ? t + ((left[d] * ls[d].weight) / rate) * HOUR : Infinity;
    const to = Math.min(dry, soonest, until, typeof pace === 'number' ? Infinity : (Math.floor(t / HOUR) + 1) * HOUR);
    if (d !== null && rate > 0) left[d] = Math.max(0, left[d] - (((to - t) / HOUR) * rate) / ls[d].weight);
    t = to;
    points.push([t, pool()]);
    if (d !== null && t === dry) {
      left[d] = 0;
      if (!firstEmpty) firstEmpty = { at: t, name: ls[d].name };
      if (poolEmpty === null && left.every((x) => x === 0)) poolEmpty = t;
    }
    let refilled = false;
    ls.forEach((l, i) => {
      const n = next[i];
      if (n === null || n > t) return;
      if (left[i] < 100) refills.push({ at: t, gain: l.weight * (100 - left[i]), name: l.name });
      left[i] = 100;
      next[i] = null;
      refilled = true;
    });
    if (refilled) points.push([t, pool()]);
  }
  return { points, refills, firstEmpty, poolEmpty };
}

const WEEK_HOURS = 168;
const FULL = 99.5;

const slotOf = (k: number) => {
  const d = new Date(k * HOUR + HOUR / 2);
  return d.getDay() * 24 + d.getHours();
};

export const rateAt = (slots: number[]) => (t: number) => slots[slotOf(Math.floor(t / HOUR))];

type Hourly = { rise: Map<number, number>; seen: Set<number>; full: Set<number> };

function hourly(l: Login): Hourly {
  const out: Hourly = { rise: new Map(), seen: new Set(), full: new Set() };
  const lastSeen = Math.max(-Infinity, ...l.past.map((i) => i.samples.at(-1)?.[0] ?? -Infinity));
  const opened = l.resetsAt !== null && l.length ? Math.max(l.resetsAt - l.length, lastSeen) : null;
  const current = { opened, samples: l.samples };
  for (const { opened, samples } of [...l.past, current]) {
    if (!samples.length) continue;
    const pts: Point[] = opened !== null && samples[0][0] > opened ? [[opened, 0], ...samples] : samples;
    for (let k = Math.ceil(pts[0][0] / HOUR); (k + 1) * HOUR <= pts.at(-1)![0]; k++) out.seen.add(k);
    for (let i = 1; i < pts.length; i++) {
      const [a, was] = pts[i - 1];
      const [b, now] = pts[i];
      if (b <= a) continue;
      if (was >= FULL) for (let k = Math.ceil(a / HOUR); (k + 1) * HOUR <= b; k++) out.full.add(k);
      if (now <= was) continue;
      for (let k = Math.floor(a / HOUR); k * HOUR < b; k++) {
        const part = (Math.min(b, (k + 1) * HOUR) - Math.max(a, k * HOUR)) / (b - a);
        out.rise.set(k, (out.rise.get(k) ?? 0) + (now - was) * part);
      }
    }
  }
  return out;
}

function average(hours: Iterable<number>, before: number, value: (k: number) => number | null): { slots: number[]; hours: number } | null {
  const sum = Array<number>(WEEK_HOURS).fill(0);
  const n = Array<number>(WEEK_HOURS).fill(0);
  for (const k of hours) {
    if (k >= before) continue;
    const v = value(k);
    if (v === null) continue;
    sum[slotOf(k)] += v;
    n[slotOf(k)]++;
  }
  const count = n.reduce((a, v) => a + v, 0);
  if (count < 24) return null;
  const all = sum.reduce((a, v) => a + v, 0) / count;
  const ofHour = (h: number) => {
    let [s, c] = [0, 0];
    for (let d = 0; d < 7; d++) [s, c] = [s + sum[d * 24 + h], c + n[d * 24 + h]];
    return c ? s / c : all;
  };
  return { slots: sum.map((v, i) => (n[i] ? v / n[i] : ofHour(i % 24))), hours: count };
}

export type Usual = { slots: number[]; days: number; week: number; own: (number[] | null)[] };

export function usual(ls: Login[], now: number): Usual | null {
  if (!ls.length) return null;
  const hs = ls.map(hourly);
  const before = Math.floor(now / HOUR);
  const known = hs.filter((h) => h.seen.size);
  if (!known.length) return null;
  const pool = average(known[0].seen, before, (k) => {
    if (!known.every((h) => h.seen.has(k)) || known.every((h) => h.full.has(k))) return null;
    return ls.reduce((a, l, i) => a + l.weight * (hs[i].rise.get(k) ?? 0), 0);
  });
  if (!pool) return null;
  return {
    slots: pool.slots,
    days: pool.hours / 24,
    week: pool.slots.reduce((a, v) => a + v, 0),
    own: hs.map((h) => average(h.seen, before, (k) => (h.full.has(k) ? null : (h.rise.get(k) ?? 0)))?.slots ?? null),
  };
}

function emptyBy(l: Login, at: number, rate: (t: number) => number): number | null {
  let left = 100 - l.used;
  const end = l.resetsAt ?? at + 14 * 24 * HOUR;
  for (let t = at; t < end; ) {
    const to = Math.min(end, (Math.floor(t / HOUR) + 1) * HOUR);
    const spend = (rate(t) * (to - t)) / HOUR;
    if (spend >= left) return spend > 0 ? t + (left / spend) * (to - t) : t;
    left -= spend;
    t = to;
  }
  return null;
}

export function pick(all: Combined[], title: string | undefined): Combined | null {
  if (!all.length) return null;
  const minutes = (c: Combined) => Math.max(0, ...c.parts.map((p) => p.window.minutes ?? 0));
  return all.find((c) => c.title === title) ?? all.reduce((a, b) => (minutes(b) > minutes(a) ? b : a));
}
