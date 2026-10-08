import { describe, expect, it } from 'vitest';
import type { Credential, Live, State } from './types';
import { behind, combine, dropping, dropsLive, overwrite, taken, usageOf } from './usage';

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

describe('usageOf', () => {
  it('takes a login its own figures when TokenGauge names it', () => {
    const usage = { windows: win(7), accounts: { work: { windows: win(42) } } };
    expect(usageOf(cred('work'), usage)!.windows[0].usedPercent).toBe(42);
    expect(usageOf(cred('perso'), usage)).toEqual({ missing: true, windows: [] });
  });

  it('falls back to the provider figures for the active login only when there are some', () => {
    const usage = { windows: win(7), accounts: { work: { windows: win(42) } } };
    expect(usageOf(cred('perso', true), usage)).toBe(usage);
    const empty = { windows: [], accounts: { work: { windows: win(42) } } };
    expect(usageOf(cred('perso', true), empty)).toEqual({ missing: true, windows: [] });
  });

  it('gives an old snapshot to the active login only', () => {
    const usage = { windows: win(7), accounts: {} };
    expect(usageOf(cred('work', true), usage)).toBe(usage);
    expect(usageOf(cred('perso'), usage)).toEqual({ passive: true, windows: [] });
    expect(usageOf(cred('perso'), null)).toBeNull();
  });

  it('does not take a name inherited from Object', () => {
    expect(usageOf(cred('toString'), { windows: [], accounts: { x: { windows: [] } } })).toEqual({
      missing: true,
      windows: [],
    });
  });
});

describe('behind', () => {
  const state = (updatedAt: number, switched: number, accounts = {}) =>
    ({
      health: { switchedAt: { claude: switched } },
      usage: { updatedAt, providers: { claude: { windows: [], accounts } } },
    }) as unknown as State;

  it('holds an unnamed snapshot older than the last switch', () => {
    expect(behind(state(1, 2), 'claude')).toBe(true);
    expect(behind(state(3, 2), 'claude')).toBe(false);
    expect(behind(state(1, 2, { work: { windows: [] } }), 'claude')).toBe(false);
  });
});

describe('confirmation texts', () => {
  const live = (l: Partial<Live>) => ({ id: 'claude', provider: 'claude', ...l }) as Live;

  it('names what a switch would drop', () => {
    expect(dropsLive(live({ state: 'stored', name: 'work', confirmed: true }))).toBe(false);
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

describe('combine', () => {
  const at = (iso: string) => ({ title: 'Weekly', usedPercent: 0, resetsAt: iso });

  it('sums a window over the logins that report it', () => {
    const usage = {
      windows: [],
      accounts: {
        work: { windows: [{ ...at('2026-10-12T00:00:00Z'), usedPercent: 81 }, ...win(64)] },
        perso: { windows: [{ ...at('2026-10-10T00:00:00Z'), usedPercent: 46 }] },
        spare: { windows: [], credentialState: 'expired' },
        gone: { windows: win(99), error: '401' },
      },
    };
    const creds = [cred('work', true), cred('perso'), cred('spare'), cred('gone')];
    const [weekly, hours] = combine(creds, usage);
    expect(weekly).toMatchObject({ title: 'Weekly', used: 127, of: 200, resetsAt: Date.parse('2026-10-10T00:00:00Z') });
    expect(weekly.parts.map((p) => [p.name, p.active])).toEqual([
      ['work', true],
      ['perso', false],
    ]);
    expect(hours).toMatchObject({ title: '5 hours', used: 64, of: 100, resetsAt: null });
  });

  it('keeps one part per login, in order, however many there are', () => {
    const names = ['a', 'b', 'c', 'd', 'e'];
    const accounts = Object.fromEntries(names.map((n, i) => [n, { windows: win(20 * i) }]));
    const [hours] = combine(
      names.map((n) => cred(n, n === 'c')),
      { windows: [], accounts },
    );
    expect(hours).toMatchObject({ used: 200, of: 500 });
    expect(hours.parts.map((p) => `${p.name}:${p.window.usedPercent}`)).toEqual(['a:0', 'b:20', 'c:40', 'd:60', 'e:80']);
    expect(hours.parts.filter((p) => p.active).map((p) => p.name)).toEqual(['c']);
  });

  it('has nothing to combine without figures', () => {
    expect(combine([cred('work', true)], null)).toEqual([]);
    expect(combine([cred('work', true)], { behind: true, windows: [] })).toEqual([]);
  });
});
