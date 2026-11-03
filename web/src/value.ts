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

export function changeText(ratio: number): string {
  if (ratio >= 1.05) return `${ratio.toFixed(1)}x the weeks before`;
  if (ratio <= 0.95) return `${Math.round((1 - ratio) * 100)}% under the weeks before`;
  return 'about the same as the weeks before';
}
