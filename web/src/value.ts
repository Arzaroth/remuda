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

export type Plan = { provider: string; login: string; account: string | null; weeks: Week[] };

// Every login's weeks, oldest first, the logins in the order their newest
// week came. Two accounts that held one name are two histories.
export function plans(weeks: Week[]): Plan[] {
  const out = new Map<string, Plan>();
  for (const w of [...weeks].sort((a, b) => b.openedAt - a.openedAt)) {
    if (!w.login) continue;
    const key = `${w.provider}/${w.login}/${w.account ?? ''}`;
    if (!out.has(key)) out.set(key, { provider: w.provider, login: w.login, account: w.account, weeks: [] });
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

export type Order = 'recent' | 'value' | 'name';

export type Filters = {
  months: number;
  minUsed: number;
  removed: boolean;
  unclaimed: boolean;
  open: boolean;
  incomplete: boolean;
  idle: boolean;
  cut: boolean;
  order: Order;
};

export const FILTERS: Filters = {
  months: 0,
  minUsed: 0,
  removed: true,
  unclaimed: false,
  open: false,
  incomplete: false,
  idle: false,
  cut: false,
  order: 'recent',
};

export function changed(f: Filters): number {
  return (Object.keys(FILTERS) as (keyof Filters)[]).filter((k) => k !== 'order' && f[k] !== FILTERS[k]).length;
}

export function removed(p: Plan, logins: Record<string, Login> | undefined): boolean {
  if (!p.account || !logins) return false;
  const l = logins[p.account];
  return !l || !!l.removed;
}

export function kept(w: Week, f: Filters, now: number): boolean {
  if (f.months > 0) {
    const since = new Date(now);
    since.setMonth(since.getMonth() - f.months);
    if (w.endsAt <= since.getTime()) return false;
  }
  if (w.used < f.minUsed) return false;
  if (f.unclaimed && w.unattributedUsd >= 0.01) return false;
  if (f.open && w.open) return false;
  if (f.incomplete && (w.partial || w.unpriced)) return false;
  if (f.idle && w.usd < 0.01) return false;
  if (f.cut && cutShort(w) != null) return false;
  return true;
}

export type Shown = { plan: Plan; weeks: Week[]; hidden: number };

// The logins with a week left after the filters, in the order asked, and
// those left with none or hidden as removed, so the page can name them.
export function arrange(
  ps: Plan[],
  f: Filters,
  logins: Record<string, Login> | undefined,
  now: number,
): { shown: Shown[]; empty: Plan[]; gone: Plan[] } {
  const gone = f.removed ? ps.filter((p) => removed(p, logins)) : [];
  const all = ps
    .filter((p) => !gone.includes(p))
    .map((plan) => {
      const weeks = plan.weeks.filter((w) => kept(w, f, now));
      return { plan, weeks, hidden: plan.weeks.length - weeks.length };
    });
  const shown = all.filter((s) => s.weeks.length);
  if (f.order === 'name') shown.sort((a, b) => a.plan.login.localeCompare(b.plan.login) || a.plan.provider.localeCompare(b.plan.provider));
  if (f.order === 'value') {
    const per = new Map(shown.map((s) => [s, rate(s.weeks)]));
    shown.sort((a, b) => (per.get(b) ?? -1) - (per.get(a) ?? -1));
  }
  return { shown, empty: all.filter((s) => !s.weeks.length).map((s) => s.plan), gone };
}
