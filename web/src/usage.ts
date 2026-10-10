import type { Combined, Credential, Live, Provider, Shown, State } from './types';

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

// The figures below are the server's (`src/shown.rs`), which adds logins up
// with `selvedge::plans`, as TokenGauge's panels do; the page only draws them.

// What a login's card shows, or null when TokenGauge has nothing for its CLI.
export function shownFor(s: State, c: Credential): Shown | null {
  const accounts = s.shown?.[c.provider]?.accounts || {};
  return Object.hasOwn(accounts, c.name) ? accounts[c.name] : null;
}

// TokenGauge's figures for a CLI, or what stands in for them.
export const providerUsage = (s: State, id: string): Shown | null => s.shown?.[id]?.usage ?? null;

// When the soonest window among the logins added up resets.
export function resetsAt(c: Combined): number | null {
  const at = c.parts.flatMap((p) => (p.window.resetsAt ? [Date.parse(p.window.resetsAt)] : []));
  return at.length ? Math.min(...at) : null;
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

