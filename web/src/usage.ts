import type { Credential, Live, Provider, Shown, State, Usage, Window } from './types';

// The CLIs worth a card and a tab: one with a stored login, or signed into
// one (an unstored login is there to import). The rest are noise until an
// account is added, which the add form still offers for every CLI. With
// none of either, all of them, or the page would be empty.
export function inUse(s: State): Provider[] {
  const used = s.providers.filter(
    (p) =>
      s.credentials.some((c) => c.provider === p.id) ||
      s.live.some((l) => l.provider === p.id && l.state !== 'signed_out'),
  );
  return used.length ? used : s.providers;
}

export function usageOf(c: Credential, usage: Shown | null): Shown | null {
  if (!usage) return null;
  const accounts = usage.accounts || {};
  if (Object.hasOwn(accounts, c.name)) return accounts[c.name];
  const named = Object.keys(accounts).length > 0;
  if (!c.active) return { [named ? 'missing' : 'passive']: true, windows: [] };
  return named && !usage.windows.length && !usage.error ? { missing: true, windows: [] } : usage;
}

export function behind(state: State, id: string): boolean {
  const u = state.usage;
  if (u?.updatedAt == null || Object.keys(u.providers?.[id]?.accounts || {}).length) return false;
  return u.updatedAt < (state.health?.switchedAt?.[id] ?? 0);
}

export function providerUsage(state: State, id: string): Shown | null {
  return behind(state, id) ? { behind: true, windows: [] } : (state.usage?.providers?.[id] as Usage) || null;
}

export const dropsLive = (live: Live | undefined) =>
  live?.state === 'foreign' || live?.state === 'unstored' || (live?.state === 'stored' && !live.confirmed);

export function dropping(live: Live): string {
  if (live.state === 'unstored') return `Drop ${live.email || 'the live login'} unsaved?`;
  if (live.state === 'foreign') return `Drop the ${(live.what ?? '').replace(/^an? /, '')}?`;
  return `Lose ${live.name}'s newest tokens?`;
}

export function overwrite(creds: Credential[], provider: string, name: string): string {
  const c = creds.find((c) => c.provider === provider && c.name === name);
  return `Overwrite ${name}${c?.email ? ` (${c.email})` : ''}?`;
}

export const taken = (creds: Credential[], provider: string, name: string) =>
  creds.some((c) => c.provider === provider && c.name === name);

export type Part = { name: string; active: boolean; window: Window; weight: number };
export type Combined = {
  title: string;
  weighted: boolean;
  used: number;
  of: number;
  pooled: number;
  parts: Part[];
  widths: number[];
  leftOut: string[];
  resetsAt: number | null;
};

const roundEven = (x: number) => {
  const r = Math.round(x);
  return r - x === 0.5 && r % 2 ? r - 1 : r;
};

const MIN_SEGMENT = 0.1;

export function segmentWidths(weights: number[]): number[] {
  const n = weights.length;
  if (MIN_SEGMENT * n >= 1) return weights.map(() => 1 / n);
  const pinned = weights.map(() => false);
  for (;;) {
    const free = 1 - MIN_SEGMENT * pinned.filter(Boolean).length;
    const weight = weights.reduce((sum, w, i) => (pinned[i] ? sum : sum + w), 0);
    const widths = weights.map((w, i) => (pinned[i] ? MIN_SEGMENT : (w / weight) * free));
    let moved = false;
    widths.forEach((w, i) => {
      if (!pinned[i] && w < MIN_SEGMENT) moved = pinned[i] = true;
    });
    if (!moved) return widths;
  }
}

const shownBy = (creds: Credential[], usage: Shown | null) =>
  creds.map((c) => {
    const u = usageOf(c, usage);
    return { c, windows: u && !u.error ? u.windows : [], weight: u?.planWeight ?? null };
  });

export const weighable = (creds: Credential[], usage: Shown | null) =>
  shownBy(creds, usage).some((s) => s.weight && s.windows.length);

export function combine(creds: Credential[], usage: Shown | null, weighted: boolean): Combined[] {
  const shown = shownBy(creds, usage);
  const titles = [...new Set(shown.flatMap((s) => s.windows.map((w) => w.title)))];
  return titles.map((title) => {
    const reporting = shown.flatMap(({ c, windows, weight }) => {
      const window = windows.find((w) => w.title === title);
      return window ? [{ c, window, weight }] : [];
    });
    const byWeight = weighted && reporting.some((r) => r.weight);
    const leftOut = byWeight ? reporting.filter((r) => !r.weight).map((r) => r.c.name) : [];
    const parts = reporting
      .filter((r) => !byWeight || r.weight)
      .map(({ c, window, weight }) => ({ name: c.name, active: c.active, window, weight: byWeight ? weight! : 1 }));
    const largest = Math.max(...parts.map((p) => p.weight));
    const total = parts.reduce((sum, p) => sum + p.weight, 0);
    const sum = parts.reduce((sum, p) => sum + p.window.usedPercent * p.weight, 0);
    const resets = parts.flatMap((p) => (p.window.resetsAt ? [Date.parse(p.window.resetsAt)] : []));
    return {
      title,
      weighted: byWeight,
      used: roundEven(sum / largest),
      of: roundEven((total / largest) * 100),
      pooled: sum / total,
      parts,
      widths: segmentWidths(parts.map((p) => p.weight)),
      leftOut,
      resetsAt: resets.length ? Math.min(...resets) : null,
    };
  });
}
