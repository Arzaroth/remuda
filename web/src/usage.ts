import type { Credential, Live, Shown, State, Usage, Window } from './types';

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

export type Part = { name: string; active: boolean; window: Window };
export type Combined = { title: string; used: number; of: number; parts: Part[]; resetsAt: number | null };

export function combine(creds: Credential[], usage: Shown | null): Combined[] {
  const shown = creds.map((c) => {
    const u = usageOf(c, usage);
    return { c, windows: u && !u.error ? u.windows : [] };
  });
  const titles = [...new Set(shown.flatMap((s) => s.windows.map((w) => w.title)))];
  return titles.map((title) => {
    const parts = shown.flatMap(({ c, windows }) => {
      const window = windows.find((w) => w.title === title);
      return window ? [{ name: c.name, active: c.active, window }] : [];
    });
    const resets = parts.flatMap((p) => (p.window.resetsAt ? [Date.parse(p.window.resetsAt)] : []));
    return {
      title,
      used: parts.reduce((sum, p) => sum + p.window.usedPercent, 0),
      of: parts.length * 100,
      parts,
      resetsAt: resets.length ? Math.min(...resets) : null,
    };
  });
}
