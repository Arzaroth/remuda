export type Day = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';
export type Schedule = { days?: Day[]; hours?: string };
export type Rule = { when: string; order: string[][]; reserve?: Record<string, string> };
export type Auto = {
  enabled?: boolean;
  dwell?: string;
  balance?: boolean;
  margin?: number;
  logins?: string[];
  rule?: Rule[];
  ceiling?: number;
  ceilings?: Record<string, number>;
  long_ceilings?: Record<string, number>;
  fallback?: string;
  fallback_when_maxed?: boolean;
};

/** A percentage typed into a field: whole, 1 to 100, or none. */
export function percent(text: string): number | null {
  const n = Math.round(Number(text));
  return text.trim() && Number.isFinite(n) ? Math.min(100, Math.max(1, n)) : null;
}
export type Config = {
  schedules?: Record<string, Schedule>;
  auto?: Record<string, Auto>;
  warm?: Record<string, string[]>;
};
export type Problem = { at: string; message: string };
export type Loaded = { config: Config; version: string; problems: Problem[]; path: string };

export const OTHERWISE = 'otherwise';
export const DAYS: [Day, string][] = [
  ['mon', 'Mon'],
  ['tue', 'Tue'],
  ['wed', 'Wed'],
  ['thu', 'Thu'],
  ['fri', 'Fri'],
  ['sat', 'Sat'],
  ['sun', 'Sun'],
];

const OPTIONAL_TABLES = new Set(['reserve', 'schedules', 'auto', 'ceilings', 'long_ceilings', 'warm']);
const unset = (k: string, v: unknown): boolean =>
  v === undefined ||
  v === false ||
  (Array.isArray(v) && !v.length) ||
  (OPTIONAL_TABLES.has(k) && !!v && typeof v === 'object' && !Object.keys(v).length);

/** The same text for two configs the server writes the same. */
export function canonical(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonical).join(',')}]`;
  if (v && typeof v === 'object') {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o)
      .filter((k) => !unset(k, o[k]))
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonical(o[k])}`)
      .join(',')}}`;
  }
  return JSON.stringify(v);
}

export function minutes(span: string | undefined): number {
  if (!span) return 30;
  let total = 0;
  for (const [, n, unit] of span.matchAll(/(\d+)([hms])/g)) total += Number(n) * ({ h: 3600, m: 60, s: 1 }[unit] ?? 0);
  return Math.round(total / 60);
}

export function span(mins: number): string {
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return (h ? `${h}h` : '') + (m || !h ? `${m}m` : '');
}

export function hours(h: string | undefined): { start: string; end: string } | null {
  const m = h?.match(/^(\d{1,2}:\d{2})-(\d{1,2}:\d{2})$/);
  return m ? { start: m[1].padStart(5, '0'), end: m[2] === '24:00' ? '00:00' : m[2].padStart(5, '0') } : null;
}

export function auto(c: Config, provider: string): Auto {
  return (c.auto ??= {})[provider] ??= {};
}

export function scheduleUses(c: Config, name: string): string[] {
  const uses: string[] = [];
  for (const [provider, a] of Object.entries(c.auto ?? {}))
    (a.rule ?? []).forEach((r, i) => {
      if (r.when === name || Object.values(r.reserve ?? {}).includes(name)) uses.push(`${provider} rule ${i + 1}`);
    });
  return uses;
}

export function renameSchedule(c: Config, from: string, to: string) {
  const all = (c.schedules ??= {});
  if (from === to || !(from in all) || to in all) return;
  c.schedules = Object.fromEntries(Object.entries(all).map(([k, v]) => [k === from ? to : k, v]));
  for (const a of Object.values(c.auto ?? {}))
    for (const r of a.rule ?? []) {
      if (r.when === from) r.when = to;
      for (const [login, s] of Object.entries(r.reserve ?? {})) if (s === from) r.reserve![login] = to;
    }
}

export function scheduleName(c: Config, name: string): string | null {
  if (!/^[A-Za-z0-9_-]+$/.test(name)) return 'letters, digits, - and _ only';
  if (name === OTHERWISE) return `"${OTHERWISE}" is kept for the catch-all rule`;
  if (name in (c.schedules ?? {})) return 'a schedule has that name';
  return null;
}

export function inRule(r: Rule): string[] {
  return r.order.flat();
}

export function dropFromRule(r: Rule, login: string) {
  r.order = r.order.map((t) => t.filter((n) => n !== login));
  if (r.reserve) delete r.reserve[login];
}

export function problemsAt(problems: Problem[], prefix: string): Problem[] {
  return problems.filter((p) => p.at === prefix || p.at.startsWith(`${prefix}.`) || p.at.startsWith(`${prefix}[`));
}

export type Status = import('./types').AutoStatus;

/** What a CLI's last round came to, in a few words, and how it reads. */
export function headline(s: Status, active: string | null): { text: string; tone: 'good' | 'warn' | 'bad' | '' } {
  const a = s.decision.action;
  if (s.error) return { text: `Could not switch to ${a.to}`, tone: 'bad' };
  if (!s.enabled)
    return {
      text: a.kind === 'switch' ? `Off; it would switch to ${a.to}` : a.kind === 'hold' ? 'Off; it would hold' : 'Off',
      tone: '',
    };
  if (a.kind === 'hold') return { text: 'Holding', tone: 'warn' };
  if (a.kind === 'switch')
    return { text: s.switched?.at === s.at ? `Switched to ${a.to}` : `Switching to ${a.to}`, tone: 'good' };
  return { text: active ? `Stays on ${active}` : 'Stays', tone: 'good' };
}

/** Adds `name` to the logins kept warm for `provider`, or takes it off. */
export function toggleWarm(c: Config, provider: string, name: string) {
  const all = (c.warm ??= {});
  const list = (all[provider] ??= []);
  const i = list.indexOf(name);
  if (i >= 0) list.splice(i, 1);
  else list.push(name);
  if (!list.length) delete all[provider];
}

/** The logins in the order auto mode tries them, then those it may not pick. */
export function ordered(a: Auto, names: string[]): string[] {
  const picked = (a.logins ?? []).filter((n) => names.includes(n));
  return [...picked, ...names.filter((n) => !picked.includes(n))];
}

/** Moves a picked login `by` places in the order. */
export function moveLogin(a: Auto, name: string, by: number) {
  const l = a.logins ?? [];
  const i = l.indexOf(name);
  const j = i + by;
  if (i < 0 || j < 0 || j >= l.length) return;
  l.splice(j, 0, ...l.splice(i, 1));
}

/** Lets auto mode pick a login, last in the order, or stops it. */
export function toggleLogin(a: Auto, name: string) {
  const l = (a.logins ??= []);
  const i = l.indexOf(name);
  if (i >= 0) l.splice(i, 1);
  else l.push(name);
  if (!l.length) delete a.logins;
}

export type Ceilings = 'ceilings' | 'long_ceilings';

/** Sets a login's own ceiling in `tables`, or drops it with `c` null. */
export function setCeiling(a: Auto, tables: Ceilings[], name: string, c: number | null) {
  for (const t of tables) {
    if (c !== null) (a[t] ??= {})[name] = c;
    else if (a[t]) {
      delete a[t]![name];
      if (!Object.keys(a[t]!).length) delete a[t];
    }
  }
}

/**
 * A login's one ceiling for every window, when its two entries agree or it
 * has one window (`single`), which takes the long entry; else null.
 */
export function oneCeiling(a: Auto, name: string, single: string[] = []): number | undefined | null {
  const short = a.ceilings?.[name];
  const long = a.long_ceilings?.[name];
  return single.includes(name) ? long : short === long ? short : null;
}

/** What only the advanced settings show, in a few words each. */
export function advancedOnly(a: Auto, names: string[], single: string[] = []): string[] {
  const out: string[] = [];
  const rules = a.rule?.length ?? 0;
  if (rules) out.push(rules === 1 ? 'a rule' : `${rules} rules`);
  if (a.fallback) out.push(`the fallback to ${a.fallback}`);
  if (a.balance) out.push('ranking by headroom before a reset');
  for (const n of names) if (oneCeiling(a, n, single) === null) out.push(`${n}'s two ceilings`);
  return out;
}

/**
 * Drops what simple mode cannot show. A login's two ceilings become the
 * lower, or the long one for a login with one window.
 */
export function simplify(a: Auto, names: string[], single: string[] = []) {
  delete a.rule;
  delete a.fallback;
  delete a.fallback_when_maxed;
  delete a.balance;
  delete a.margin;
  for (const n of names)
    if (single.includes(n)) setCeiling(a, ['ceilings', 'long_ceilings'], n, a.long_ceilings?.[n] ?? null);
    else if (oneCeiling(a, n) === null) {
      const both = [a.ceilings?.[n], a.long_ceilings?.[n]].filter((c): c is number => c !== undefined);
      setCeiling(a, ['ceilings', 'long_ceilings'], n, Math.min(...both));
    }
}
