import type { Login } from './spend';

export type Week = {
  provider: string;
  login: string | null;
  account: string | null;
  openedAt: number;
  endsAt: number;
  open: boolean;
  used: number;
  usd: number;
  usdPerPercent: number | null;
  unattributedUsd: number;
  partial: boolean;
  unpriced: boolean;
  unattributedUnpriced?: boolean;
};

export type ValueAnswer = { weeks?: Week[]; logins?: Record<string, Login>; unavailable?: string };

export type Plan = { provider: string; login: string; weeks: Week[] };

// Every login's weeks, oldest first, the logins in the order their newest
// week came. Two accounts that held one name are two histories.
export function plans(weeks: Week[]): Plan[] {
  const out = new Map<string, Plan>();
  for (const w of [...weeks].sort((a, b) => b.openedAt - a.openedAt)) {
    if (!w.login) continue;
    const key = `${w.provider}/${w.login}/${w.account ?? ''}`;
    if (!out.has(key)) out.set(key, { provider: w.provider, login: w.login, weeks: [] });
    out.get(key)!.weeks.unshift(w);
  }
  return [...out.values()];
}

// How the latest closed week's dollars per percent compare with the closed
// weeks before it (four at most): 2 is twice as much, 0.5 half. Null without
// two closed weeks with a whole, priced figure.
export function change(weeks: Week[]): number | null {
  const closed = weeks.filter((w) => !w.open && w.usdPerPercent != null && !w.partial && !w.unpriced);
  if (closed.length < 2) return null;
  const last = closed[closed.length - 1].usdPerPercent!;
  const before = closed.slice(-5, -1).map((w) => w.usdPerPercent!);
  const mean = before.reduce((a, b) => a + b, 0) / before.length;
  return mean > 0 ? last / mean : null;
}

const DAY = 86_400_000;

// The days a week ran when a reset closed it before its seventh.
export function cutShort(w: Week): number | null {
  if (w.open) return null;
  const days = (w.endsAt - w.openedAt) / DAY;
  return days < 7 - 1 / 24 ? days : null;
}

export function changeText(ratio: number): string {
  if (ratio >= 1.05) return `${ratio.toFixed(1)}x the weeks before`;
  if (ratio <= 0.95) return `${Math.round((1 - ratio) * 100)}% under the weeks before`;
  return 'about the same as the weeks before';
}

export function rate(weeks: Week[], last = 4): number | null {
  const closed = weeks
    .filter((w) => !w.open && !w.partial && !w.unpriced && w.used > 0 && !w.unattributedUnpriced && w.unattributedUsd <= w.usd / 100)
    .slice(-last);
  const used = closed.reduce((a, w) => a + w.used, 0);
  return used > 0 ? closed.reduce((a, w) => a + w.usd, 0) / used : null;
}

export type Weight = { advertised: number; measured: number };

export function weights(ps: Plan[], advertised: Record<string, Record<string, number>>): Map<Plan, Weight> {
  const out = new Map<Plan, Weight>();
  for (const [provider, given] of Object.entries(advertised)) {
    const top = Math.max(0, ...Object.values(given));
    if (top <= 0) continue;
    const seen = new Set<string>();
    const rated = ps
      .filter((p) => p.provider === provider && !seen.has(p.login) && seen.add(p.login))
      .map((p) => ({ plan: p, weight: given[p.login], rate: rate(p.weeks) }))
      .filter((r): r is { plan: Plan; weight: number; rate: number } => r.weight > 0 && r.rate !== null && r.rate > 0);
    const ref = rated.find((r) => r.weight === top);
    if (rated.length < 2 || !ref) continue;
    for (const r of rated) out.set(r.plan, { advertised: r.weight / top, measured: r.rate / ref.rate });
  }
  return out;
}
