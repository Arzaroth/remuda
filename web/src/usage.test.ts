import { describe, expect, it } from 'vitest';
import type { Credential, Live, State } from './types';
import {
  behind,
  combine,
  dropping,
  dropsLive,
  overwrite,
  segmentWidths,
  taken,
  usageOf,
  weighable,
} from './usage';

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
    const [weekly, hours] = combine(creds, usage, false);
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
      false,
    );
    expect(hours).toMatchObject({ used: 200, of: 500 });
    expect(hours.parts.map((p) => `${p.name}:${p.window.usedPercent}`)).toEqual(['a:0', 'b:20', 'c:40', 'd:60', 'e:80']);
    expect(hours.parts.filter((p) => p.active).map((p) => p.name)).toEqual(['c']);
  });

  it('has nothing to combine without figures', () => {
    expect(combine([cred('work', true)], null, true)).toEqual([]);
    expect(combine([cred('work', true)], { behind: true, windows: [] }, true)).toEqual([]);
  });

  const weighed = {
    windows: [],
    accounts: {
      perso: { windows: [{ title: 'Session', usedPercent: 31, resetsAt: null }], planWeight: 20 },
      work: { windows: [{ title: 'Session', usedPercent: 100, resetsAt: null }], planWeight: 1 },
      odd: { windows: [{ title: 'Session', usedPercent: 50, resetsAt: null }] },
    },
  };
  const three = [cred('perso', true), cred('work'), cred('odd')];

  it('weighs logins by plan in units of the largest, as TokenGauge does', () => {
    const [session] = combine(three, weighed, true);
    expect(session).toMatchObject({ used: 36, of: 105, leftOut: ['odd'] });
    expect(session.pooled).toBeCloseTo((31 * 20 + 100) / 21);
    expect(session.parts.map((p) => [p.name, p.weight])).toEqual([
      ['perso', 20],
      ['work', 1],
    ]);
    expect(weighable(three, weighed)).toBe(true);
    expect(session.widths[0]).toBeCloseTo(0.9);
    expect(session.widths[1]).toBeCloseTo(0.1);
  });

  it('counts every login once when absolute, or when no plan has a weight', () => {
    const [session] = combine(three, weighed, false);
    expect(session).toMatchObject({ used: 181, of: 300, leftOut: [] });
    expect(session.parts.every((p) => p.weight === 1)).toBe(true);
    const unweighed = { windows: [], accounts: { odd: weighed.accounts.odd } };
    expect(weighable([cred('odd')], unweighed)).toBe(false);
    expect(combine([cred('odd')], unweighed, true)[0]).toMatchObject({ used: 50, of: 100, leftOut: [] });
  });

  it('keeps a window only logins without a weight report, counted once', () => {
    const extra = {
      windows: [],
      accounts: {
        perso: weighed.accounts.perso,
        odd: { windows: [{ title: 'Opus', usedPercent: 40, resetsAt: null }] },
      },
    };
    const [session, opus] = combine([cred('perso', true), cred('odd')], extra, true);
    expect(session).toMatchObject({ title: 'Session', weighted: true, used: 31, of: 100 });
    expect(opus).toMatchObject({ title: 'Opus', weighted: false, used: 40, of: 100, leftOut: [] });
  });

  it('rounds a half to even, as TokenGauge prints it', () => {
    const tie = {
      windows: [],
      accounts: {
        max: { windows: [{ title: 'Session', usedPercent: 30, resetsAt: null }], planWeight: 20 },
        pro: { windows: [{ title: 'Session', usedPercent: 10, resetsAt: null }], planWeight: 1 },
      },
    };
    expect(combine([cred('max'), cred('pro')], tie, true)[0]).toMatchObject({ used: 30, of: 105 });
    const max = { windows: [{ title: 'Session', usedPercent: 50, resetsAt: null }], planWeight: 20 };
    const odd = { ...tie, accounts: { ...tie.accounts, max } };
    expect(combine([cred('max'), cred('pro')], odd, true)[0]).toMatchObject({ used: 50 });
  });
});

describe('fractional weights', () => {
  it('weighs a Team seat as the 1.25x of a Pro it is sold as', () => {
    const usage = {
      windows: [],
      accounts: {
        max: { windows: [{ title: 'Session', usedPercent: 100, resetsAt: null }], planWeight: 20 },
        team: { windows: [{ title: 'Session', usedPercent: 100, resetsAt: null }], planWeight: 1.25 },
      },
    };
    expect(combine([cred('max'), cred('team')], usage, true)[0]).toMatchObject({ used: 106, of: 106 });
  });
});

describe('segmentWidths', () => {
  const close = (got: number[], want: number[]) => got.forEach((w, i) => expect(w).toBeCloseTo(want[i]));

  it('keeps a small plan a tenth of the bar, as TokenGauge does', () => {
    close(segmentWidths([20, 1, 1]), [0.8, 0.1, 0.1]);
    close(segmentWidths([20, 5, 1]), [0.72, 0.18, 0.1]);
    close(segmentWidths([20, 20, 20, 5, 5, 1]), [7 / 30, 7 / 30, 7 / 30, 0.1, 0.1, 0.1]);
  });

  it('shares the bar evenly when ten or more logins would each need a tenth', () => {
    close(segmentWidths(Array(12).fill(1)), Array(12).fill(1 / 12));
    close(segmentWidths([20, ...Array(9).fill(1)]), Array(10).fill(0.1));
    close(segmentWidths([5]), [1]);
    expect(segmentWidths([])).toEqual([]);
  });
});
