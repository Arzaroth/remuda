export type Tokens = { i?: number; o?: number; cw5?: number; cw1h?: number; cr?: number };
export type TypeCosts = { input: number; output: number; cacheWrite: number; cacheRead: number };

export type SpendRow = {
  date: string;
  provider: string;
  model: string;
  login: string | null;
  source: 'cli' | 'proxy';
  sessions?: number;
  tokens: Tokens;
  usd: TypeCosts | null;
};

export type Login = { provider: string; name: string; label?: string | null };

export type SpendAnswer = {
  export?: { generatedAt: number; coversFrom?: string; rows: SpendRow[] };
  logins?: Record<string, Login>;
  unavailable?: string;
};

export type Split = { input: number; output: number; cacheWrite: number; cacheRead: number };

export type Sum = {
  usd: number;
  unpriced: boolean;
  costs: Split;
  tokens: Split;
  sessions: number;
};

export const UNATTRIBUTED = '';

export function split(t: Tokens): Split {
  return { input: t.i ?? 0, output: t.o ?? 0, cacheWrite: (t.cw5 ?? 0) + (t.cw1h ?? 0), cacheRead: t.cr ?? 0 };
}

export const total = (s: Split) => s.input + s.output + s.cacheWrite + s.cacheRead;

const empty = (): Sum => ({
  usd: 0,
  unpriced: false,
  costs: { input: 0, output: 0, cacheWrite: 0, cacheRead: 0 },
  tokens: { input: 0, output: 0, cacheWrite: 0, cacheRead: 0 },
  sessions: 0,
});

function add(into: Sum, row: SpendRow) {
  const t = split(row.tokens);
  for (const k of ['input', 'output', 'cacheWrite', 'cacheRead'] as const) {
    into.tokens[k] += t[k];
    if (row.usd) into.costs[k] += row.usd[k];
  }
  if (row.usd) into.usd += row.usd.input + row.usd.output + row.usd.cacheWrite + row.usd.cacheRead;
  else if (total(t) > 0) into.unpriced = true;
  into.sessions += row.sessions ?? 0;
}

export function sum(rows: SpendRow[]): Sum {
  const s = empty();
  for (const r of rows) add(s, r);
  return s;
}

/** Rows summed per key, largest spend first. */
export function by(rows: SpendRow[], key: (r: SpendRow) => string): [string, Sum][] {
  const groups = new Map<string, Sum>();
  for (const r of rows) {
    const k = key(r);
    if (!groups.has(k)) groups.set(k, empty());
    add(groups.get(k)!, r);
  }
  return [...groups].sort((a, b) => b[1].usd - a[1].usd || total(b[1].tokens) - total(a[1].tokens));
}

/** Every local date from `from` to `to`, inclusive, as `YYYY-MM-DD`. */
export function dates(from: string, to: string): string[] {
  const out: string[] = [];
  const day = new Date(`${from}T12:00:00`);
  const end = new Date(`${to}T12:00:00`);
  while (day <= end && out.length < 800) {
    out.push(iso(day));
    day.setDate(day.getDate() + 1);
  }
  return out;
}

export const iso = (d: Date) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;

/** The first day of a range `days` long ending `today`. */
export function since(days: number, today = new Date()): string {
  const d = new Date(today);
  d.setHours(12, 0, 0, 0);
  d.setDate(d.getDate() - (days - 1));
  return iso(d);
}

/** One value per date per series, for a chart over `days`. */
export function daily(
  rows: SpendRow[],
  days: string[],
  key: (r: SpendRow) => string,
  value: (r: SpendRow) => number,
): Map<string, number[]> {
  const index = new Map(days.map((d, i) => [d, i]));
  const out = new Map<string, number[]>();
  for (const r of rows) {
    const i = index.get(r.date);
    if (i === undefined) continue;
    const k = key(r);
    if (!out.has(k)) out.set(k, new Array(days.length).fill(0));
    out.get(k)![i] += value(r);
  }
  return out;
}

export const usdOf = (r: SpendRow) =>
  r.usd ? r.usd.input + r.usd.output + r.usd.cacheWrite + r.usd.cacheRead : 0;

/** The share of prompt tokens read from the cache. */
export function cacheHit(t: Split): number | null {
  const prompt = t.input + t.cacheRead + t.cacheWrite;
  return prompt > 0 ? t.cacheRead / prompt : null;
}

export function loginName(digest: string | null, logins: Record<string, Login> | undefined): string {
  if (!digest) return 'Unattributed';
  const l = logins?.[digest];
  return l ? (l.label ? `${l.label} (${l.name})` : l.name) : 'A login no longer stored';
}

export function money(usd: number): string {
  const digits = usd !== 0 && Math.abs(usd) < 1 ? 3 : 2;
  return usd.toLocaleString(undefined, { style: 'currency', currency: 'USD', minimumFractionDigits: digits, maximumFractionDigits: digits });
}

export function compact(n: number): string {
  for (const [size, unit] of [
    [1e12, 'T'],
    [1e9, 'B'],
    [1e6, 'M'],
    [1e3, 'K'],
  ] as const) {
    if (Math.abs(n) >= size) {
      const v = n / size;
      return `${v >= 100 ? Math.round(v) : v.toFixed(1).replace(/\.0$/, '')}${unit}`;
    }
  }
  return String(Math.round(n));
}

export const percent = (f: number) => (f > 0 && f < 0.001 ? '<0.1%' : `${(f * 100).toFixed(1).replace(/\.0$/, '')}%`);
