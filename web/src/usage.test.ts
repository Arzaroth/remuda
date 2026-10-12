import { describe, expect, it } from 'vitest';
import type { Combined, Credential, Live, State } from './types';
import { dropping, dropsLive, inUse, overwrite, resetsAt, shownFor, taken } from './usage';

const cred = (name: string, active = false): Credential => ({
  id: `claude/${name}`,
  provider: 'claude',
  name,
  label: null,
  email: `${name}@example.com`,
  plan: 'max',
  active,
  expiresAt: null,
  refreshTokenExpiresAt: null,
  verified: true,
});
const win = (usedPercent: number) => [{ title: '5 hours', usedPercent, resetsAt: null }];

describe('inUse', () => {
  const state = (credentials: Credential[], live: Partial<Live>[]) =>
    ({
      providers: [
        { id: 'claude', name: 'Claude Code' },
        { id: 'grok', name: 'Grok' },
        { id: 'glm', name: 'GLM' },
      ],
      credentials,
      live: live as Live[],
    }) as unknown as State;

  it('drops a CLI with no stored login that is signed out', () => {
    const s = state([cred('perso', true)], [
      { provider: 'claude', state: 'stored' },
      { provider: 'grok', state: 'unstored' },
      { provider: 'glm', state: 'signed_out' },
    ]);
    expect(inUse(s).map((p) => p.id)).toEqual(['claude', 'grok']);
  });

  it('keeps a CLI with a stored login while it is signed out', () => {
    const s = state([cred('perso')], [
      { provider: 'claude', state: 'signed_out' },
      { provider: 'grok', state: 'signed_out' },
    ]);
    expect(inUse(s).map((p) => p.id)).toEqual(['claude']);
  });

  it('shows every CLI before any is in use', () => {
    const s = state([], [{ provider: 'claude', state: 'signed_out' }]);
    expect(inUse(s).map((p) => p.id)).toEqual(['claude', 'grok', 'glm']);
  });
});

describe('confirmation texts', () => {
  const live = (l: Partial<Live>) => ({ id: 'claude', provider: 'claude', ...l }) as Live;

  it('names what a switch would drop', () => {
    expect(dropsLive(live({ state: 'stored', name: 'work', confirmed: true }))).toBe(false);
    expect(dropsLive(live({ state: 'stored', name: 'work', confirmed: false }))).toBe(true);
    expect(dropsLive(live({ state: 'unstored' }))).toBe(true);
    expect(dropsLive(live({ state: 'foreign' }))).toBe(true);
    expect(dropsLive(live({ state: 'signed_out' }))).toBe(false);
    expect(dropping(live({ state: 'unstored', email: 'a@b.c' }))).toBe('Drop a@b.c unsaved?');
    expect(dropping(live({ state: 'foreign', what: 'an API key' }))).toBe('Drop the API key?');
    expect(dropping(live({ state: 'stored', name: 'work', confirmed: false }))).toBe("Lose work's newest tokens?");
  });

  it('names the login an import or sign-in would overwrite', () => {
    const creds = [cred('work')];
    expect(taken(creds, 'claude', 'work')).toBe(true);
    expect(taken(creds, 'codex', 'work')).toBe(false);
    expect(overwrite(creds, 'claude', 'work')).toBe('Overwrite work (work@example.com)?');
  });
});

describe('shownFor', () => {
  const state = { shown: { claude: { accounts: { work: { windows: win(42) } } } } } as unknown as State;

  it('reads a login its card from the server', () => {
    expect(shownFor(state, cred('work'))!.windows[0].usedPercent).toBe(42);
    expect(shownFor(state, cred('perso'))).toBeNull();
  });

  it('does not take a name inherited from Object', () => {
    expect(shownFor(state, cred('toString'))).toBeNull();
  });
});

describe('resetsAt', () => {
  it('is the soonest reset among the logins added up', () => {
    const part = (resetsAt: string | null) => ({ name: 'a', active: false, weight: 1, window: { title: 'Weekly', usedPercent: 0, resetsAt } });
    const c = { parts: [part('2026-10-12T00:00:00Z'), part(null), part('2026-10-10T00:00:00Z')] } as unknown as Combined;
    expect(resetsAt(c)).toBe(Date.parse('2026-10-10T00:00:00Z'));
    expect(resetsAt({ parts: [part(null)] } as unknown as Combined)).toBeNull();
  });
});
