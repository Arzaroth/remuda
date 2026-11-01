import { iso, split, total, type Split, type Tokens } from './spend';

export type Outcome = 'ok' | 'failed' | 'canceled';

export type RequestRow = {
  hour: number;
  provider: string;
  model: string;
  login: string | null;
  key: string | null;
  outcome: Outcome;
  requests: number;
  tokens: Tokens;
  reasoning: number;
  latencyMs: number;
  generatingMs: number;
  generated: number;
};

export type RequestsAnswer = { since: number; rows: RequestRow[] };

export type Tally = {
  requests: number;
  ok: number;
  failed: number;
  canceled: number;
  tokens: Split;
  reasoning: number;
  latencyMs: number;
  generatingMs: number;
  generated: number;
};

const empty = (): Tally => ({
  requests: 0,
  ok: 0,
  failed: 0,
  canceled: 0,
  tokens: { input: 0, output: 0, cacheWrite: 0, cacheRead: 0 },
  reasoning: 0,
  latencyMs: 0,
  generatingMs: 0,
  generated: 0,
});

function add(into: Tally, r: RequestRow) {
  into.requests += r.requests;
  into[r.outcome] += r.requests;
  const t = split(r.tokens);
  for (const k of ['input', 'output', 'cacheWrite', 'cacheRead'] as const) into.tokens[k] += t[k];
  into.reasoning += r.reasoning;
  into.latencyMs += r.latencyMs;
  into.generatingMs += r.generatingMs;
  into.generated += r.generated;
}

export function tally(rows: RequestRow[]): Tally {
  const t = empty();
  for (const r of rows) add(t, r);
  return t;
}

/** Rows tallied per key, most tokens first. */
export function tallyBy(rows: RequestRow[], key: (r: RequestRow) => string): [string, Tally][] {
  const groups = new Map<string, Tally>();
  for (const r of rows) {
    const k = key(r);
    if (!groups.has(k)) groups.set(k, empty());
    add(groups.get(k)!, r);
  }
  return [...groups].sort((a, b) => total(b[1].tokens) - total(a[1].tokens) || b[1].requests - a[1].requests);
}

/** Successes over the requests that ended either way: a canceled one says nothing about the provider. */
export const successRate = (t: Tally) => (t.ok + t.failed > 0 ? t.ok / (t.ok + t.failed) : null);

/** Output tokens per second while an answer was being written. */
export const tps = (t: Tally) => (t.generatingMs > 0 ? t.generated / (t.generatingMs / 1000) : null);

export const averageLatency = (t: Tally) => (t.requests > 0 ? t.latencyMs / t.requests : null);

export const dayOf = (hour: number) => iso(new Date(hour));
