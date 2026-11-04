import { HOUR, MIN } from './format';
import type { Combined } from './types';

export type Login = {
  name: string;
  weight: number;
  used: number;
  resetsAt: number | null;
  length: number | null;
  samples: [number, number][];
};

export type Point = [number, number];
export type Refill = { at: number; gain: number; name: string };

export function logins(c: Combined, now: number): Login[] {
  const top = Math.max(...c.parts.map((p) => p.weight), 0) || 1;
  return c.parts.map((p) => {
    const at = p.window.resetsAt ? Date.parse(p.window.resetsAt) : NaN;
    const trail = (p.window.trail ?? []).filter(([t]) => t <= now);
    return {
      name: p.name,
      weight: p.weight / top,
      used: p.window.usedPercent,
      resetsAt: Number.isFinite(at) ? at : null,
      length: p.window.minutes ? p.window.minutes * MIN : null,
      samples: [...trail, [now, p.window.usedPercent]],
    };
  });
}

export const capacity = (ls: Login[]) => ls.reduce((a, l) => a + l.weight * 100, 0);

export const remaining = (ls: Login[]) => ls.reduce((a, l) => a + l.weight * (100 - l.used), 0);

export const knownFrom = (ls: Login[]) => Math.max(...ls.map((l) => l.samples[0][0]));

function usedAt(l: Login, t: number): number {
  let used = l.samples[0][1];
  for (const [at, u] of l.samples) {
    if (at > t) break;
    used = u;
  }
  return used;
}

export function recorded(ls: Login[]): { points: Point[]; refills: Refill[] } {
  if (!ls.length) return { points: [], refills: [] };
  const start = knownFrom(ls);
  const times = [...new Set(ls.flatMap((l) => l.samples.map(([t]) => t)).filter((t) => t >= start))].sort((a, b) => a - b);
  const refills: Refill[] = [];
  const points = times.map((t): Point => {
    for (const l of ls) {
      const i = l.samples.findIndex(([at]) => at === t);
      if (i > 0 && t > start && l.samples[i][1] < l.samples[i - 1][1])
        refills.push({ at: t, gain: l.weight * (l.samples[i - 1][1] - l.samples[i][1]), name: l.name });
    }
    return [t, ls.reduce((a, l) => a + l.weight * (100 - usedAt(l, t)), 0)];
  });
  return { points, refills };
}

export function burn(ls: Login[], at: number, hours: number): number | null {
  if (!ls.length) return null;
  const from = Math.max(at - hours * HOUR, knownFrom(ls));
  if (at - from < 15 * MIN) return null;
  let spent = 0;
  for (const l of ls) {
    let held = usedAt(l, from);
    for (const [t, u] of l.samples) {
      if (t <= from) continue;
      if (t > at) break;
      if (u > held) spent += l.weight * (u - held);
      held = u;
    }
  }
  return spent / ((at - from) / HOUR);
}

export type Projection = {
  points: Point[];
  refills: Refill[];
  firstEmpty: { at: number; name: string } | null;
  poolEmpty: number | null;
};

export function project(ls: Login[], now: number, rate: number, until: number): Projection {
  const left = ls.map((l) => 100 - l.used);
  const next = ls.map((l) => l.resetsAt);
  const pool = () => ls.reduce((a, l, i) => a + l.weight * left[i], 0);
  const points: Point[] = [[now, pool()]];
  const refills: Refill[] = [];
  let firstEmpty: Projection['firstEmpty'] = null;
  let poolEmpty: number | null = null;
  let t = now;
  for (let guard = 0; t < until && guard < 10_000; guard++) {
    const soonest = Math.min(...next.map((n) => n ?? Infinity));
    const open = ls.map((_, i) => i).filter((i) => left[i] > 0);
    const d = open.length ? open.reduce((a, i) => ((next[i] ?? Infinity) < (next[a] ?? Infinity) ? i : a)) : null;
    const dry = d !== null && rate > 0 ? t + ((left[d] * ls[d].weight) / rate) * HOUR : Infinity;
    const to = Math.min(dry, soonest, until);
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
      next[i] = l.length ? n + l.length : null;
      refilled = true;
    });
    if (refilled) points.push([t, pool()]);
  }
  return { points, refills, firstEmpty, poolEmpty };
}

export function pick(all: Combined[], title: string | undefined): Combined | null {
  if (!all.length) return null;
  const minutes = (c: Combined) => Math.max(0, ...c.parts.map((p) => p.window.minutes ?? 0));
  return all.find((c) => c.title === title) ?? all.reduce((a, b) => (minutes(b) > minutes(a) ? b : a));
}
