export type Day = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';
export type Schedule = { days?: Day[]; hours?: string };
export type Rule = { when: string; order: string[][]; reserve?: Record<string, string> };
export type Auto = {
  enabled?: boolean;
  dwell?: string;
  margin?: number;
  logins?: string[];
  rule?: Rule[];
  ceiling?: number;
  ceilings?: Record<string, number>;
  fallback?: string;
};

/** A percentage typed into a field: whole, 1 to 100, or none. */
export function percent(text: string): number | null {
  const n = Math.round(Number(text));
  return text.trim() && Number.isFinite(n) ? Math.min(100, Math.max(1, n)) : null;
}
export type Config = { schedules?: Record<string, Schedule>; auto?: Record<string, Auto> };
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

const OPTIONAL_TABLES = new Set(['reserve', 'schedules', 'auto', 'ceilings']);
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
