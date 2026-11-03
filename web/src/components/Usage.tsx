import { createEffect, createMemo, createSignal, For, on, onMount, Show } from 'solid-js';
import { api } from '../api';
import { brand } from '../brand';
import { now, store } from '../state';
import { href } from '../route';
import {
  by,
  cacheHit,
  compact,
  daily,
  dates,
  iso,
  loginName,
  money,
  percent,
  since,
  sum,
  total,
  usdOf,
  type Split,
  type SpendAnswer,
  type SpendRow,
  type Sum,
} from '../spend';
import {
  averageLatency,
  dayOf,
  successRate,
  tally,
  tallyBy,
  tps,
  type RequestRow,
  type RequestsAnswer,
} from '../requests';
import { Legend, LineChart, SplitBar, StackedBars, type Series } from './Charts';
import { ProviderIcon } from './ProviderIcon';
import { Value } from './Value';
import type { ValueAnswer } from '../value';

const RANGES = [
  [7, '7 days'],
  [30, '30 days'],
  [90, '90 days'],
  [365, '12 months'],
] as const;

const SLOTS = ['var(--c1)', 'var(--c2)', 'var(--c3)', 'var(--c4)', 'var(--c5)', 'var(--c6)', 'var(--c7)'];
const OTHER = 'var(--muted)';

const TYPES = [
  ['input', 'Input', 'var(--c1)'],
  ['cacheWrite', 'Cache write', 'var(--c2)'],
  ['cacheRead', 'Cache read', 'var(--c3)'],
  ['output', 'Output', 'var(--c4)'],
] as const;

const zeros = (n: number) => new Array<number>(n).fill(0);

const nameOf = (id: string) => store.s?.providers.find((p) => p.id === id)?.name ?? id;

/** A colour per model, fixed by its spend over the whole range so a filter never repaints one. */
function palette(models: string[]): (model: string) => string {
  const slots = new Map(models.slice(0, SLOTS.length).map((m, i) => [m, SLOTS[i]]));
  return (model) => slots.get(model) ?? OTHER;
}

function typeParts(s: Split, shown: (v: number) => string) {
  return TYPES.map(([k, label, color]) => ({ label, color, value: s[k], shown: shown(s[k]) }));
}

function Chips<T extends string | number | null>(props: {
  label: string;
  value: T;
  options: readonly (readonly [T, string])[];
  onPick: (v: T) => void;
}) {
  return (
    <div class="chips" role="group" aria-label={props.label}>
      <For each={props.options}>
        {([v, text]) => (
          <button type="button" class="chip" aria-pressed={props.value === v} onClick={() => props.onPick(v)}>
            {text}
          </button>
        )}
      </For>
    </div>
  );
}

function Stat(props: { label: string; value: string; note?: string }) {
  return (
    <div class="u-stat">
      <span>{props.label}</span>
      <b>{props.value}</b>
      <Show when={props.note}>
        <small>{props.note}</small>
      </Show>
    </div>
  );
}

function costOf(s: Sum) {
  return s.unpriced && s.usd === 0 ? 'unpriced' : money(s.usd) + (s.unpriced ? '+' : '');
}

function ModelDetail(props: { model: string; rows: SpendRow[]; days: string[]; color: string; all: number; onClose: () => void }) {
  const mine = createMemo(() => props.rows.filter((r) => r.model === props.model));
  const s = createMemo(() => sum(mine()));
  const tokens = () => total(s().tokens);
  const series = createMemo((): Series[] => [
    { id: props.model, label: props.model, color: props.color, values: daily(mine(), props.days, () => '', usdOf).get('') ?? props.days.map(() => 0) },
  ]);
  const hit = () => cacheHit(s().tokens);
  let card!: HTMLElement;
  onMount(() => card.scrollIntoView({ behavior: 'smooth', block: 'nearest' }));
  return (
    <section class="u-card u-detail" aria-label={props.model} ref={card}>
      <header class="u-detail-head">
        <div>
          <h2 class="spec">{props.model}</h2>
          <p class="page-sub">
            {nameOf(mine()[0]?.provider ?? '')} · {props.all > 0 ? percent(s().usd / props.all) : '0%'} of the cost
          </p>
        </div>
        <button class="btn" onClick={() => props.onClose()}>
          Close
        </button>
      </header>
      <div class="u-stats">
        <Stat label="Cost" value={costOf(s())} />
        <Stat label="Tokens" value={compact(tokens())} />
        <Stat label="Per 1M tokens" value={tokens() > 0 && !s().unpriced ? money((s().usd / tokens()) * 1e6) : '-'} />
        <Stat label="Cache hit" value={hit() === null ? '-' : percent(hit()!)} />
      </div>
      <LineChart dates={props.days} series={series()} format={money} label={`Daily cost of ${props.model}`} />
      <div class="u-two">
        <div>
          <h3>Cost by type</h3>
          <SplitBar label="Cost by type" parts={typeParts(s().costs, money)} />
        </div>
        <div>
          <h3>Tokens by type</h3>
          <SplitBar label="Tokens by type" parts={typeParts(s().tokens, compact)} />
        </div>
      </div>
    </section>
  );
}

function Spend(props: { answer: SpendAnswer; days: number; provider: string | null; login: string | null }) {
  const [model, setModel] = createSignal<string | null>(null);
  const all = () => props.answer.export?.rows ?? [];
  const range = createMemo(() => dates(since(props.days), iso(new Date())));
  const rows = createMemo(() =>
    all().filter(
      (r) => (!props.provider || r.provider === props.provider) && (props.login === null || (r.login ?? '') === props.login),
    ),
  );
  const s = createMemo(() => sum(rows()));
  const providers = createMemo(() => by(rows(), (r) => r.provider));
  const logins = createMemo(() => by(rows(), (r) => r.login ?? ''));
  const models = createMemo(() => by(rows(), (r) => r.model));
  const color = createMemo(() => palette(by(all(), (r) => r.model).map(([m]) => m)));
  const series = createMemo((): Series[] => {
    const per = daily(rows(), range(), (r) => r.provider, usdOf);
    return providers().map(([id]) => ({ id, label: nameOf(id), color: brand(id), values: per.get(id) ?? zeros(range().length) }));
  });
  const proxied = createMemo(() => sum(rows().filter((r) => r.source === 'proxy')));
  return (
    <>
      <div class="u-grid">
        <section class="u-card u-hero">
          <b class="big">{costOf(s())}</b>
          <p class="page-sub">
            {s().sessions} sessions · {compact(total(s().tokens))} tokens · priced by TokenGauge at API rates
          </p>
          <ul class="u-list">
            <For each={providers()}>
              {([id, p]) => (
                <li>
                  <ProviderIcon provider={id} size={20} />
                  <span class="u-name">
                    {nameOf(id)} <small>{p.sessions} sessions</small>
                  </span>
                  <b>{costOf(p)}</b>
                  <small class="u-sub">
                    {s().usd > 0 ? percent(p.usd / s().usd) : '-'} of cost · {compact(total(p.tokens))} tokens
                  </small>
                </li>
              )}
            </For>
          </ul>
          <Show when={proxied().usd > 0 || total(proxied().tokens) > 0}>
            <p class="page-sub">
              {costOf(proxied())} of it through the API proxy by clients that keep no transcript
            </p>
          </Show>
        </section>
        <section class="u-card">
          <h3>Daily cost</h3>
          <LineChart dates={range()} series={series()} format={money} label="Daily cost per CLI" />
          <Show when={series().length > 1}>
            <Legend series={series()} />
          </Show>
        </section>
      </div>
      <section class="u-card">
        <h3>Totals</h3>
        <div class="u-stats">
          <Stat label="Processed tokens" value={compact(total(s().tokens))} />
          <Stat label="Cached input" value={compact(s().tokens.cacheRead)} />
          <Stat label="Uncached input" value={compact(s().tokens.input + s().tokens.cacheWrite)} />
          <Stat label="Output" value={compact(s().tokens.output)} />
        </div>
        <div class="u-two">
          <div>
            <h3>Cost by type</h3>
            <SplitBar label="Cost by type" parts={typeParts(s().costs, money)} />
          </div>
          <div>
            <h3>Tokens by type</h3>
            <SplitBar label="Tokens by type" parts={typeParts(s().tokens, compact)} />
          </div>
        </div>
      </section>
      <div class="u-two">
        <section class="u-card">
          <h3>By login</h3>
          <ul class="u-list">
            <For each={logins()}>
              {([digest, l]) => {
                const known = () => props.answer.logins?.[digest];
                return (
                  <li>
                    <Show when={known()} fallback={<span class="u-dot" />}>
                      <ProviderIcon provider={known()!.provider} size={20} />
                    </Show>
                    <span class="u-name">{loginName(digest || null, props.answer.logins)}</span>
                    <b>{costOf(l)}</b>
                    <small class="u-sub">
                      {s().usd > 0 ? percent(l.usd / s().usd) : '-'} of cost · {compact(total(l.tokens))} tokens
                    </small>
                  </li>
                );
              }}
            </For>
          </ul>
          <Show when={logins().some(([d]) => !d)}>
            <p class="page-sub">Spend from before remuda recorded who spent it is unattributed.</p>
          </Show>
        </section>
        <section class="u-card">
          <h3>By model</h3>
          <table class="u-table">
            <thead>
              <tr>
                <th>Model</th>
                <th>Cost</th>
                <th>Tokens</th>
                <th>Cache hit</th>
              </tr>
            </thead>
            <tbody>
              <For each={models()}>
                {([m, x]) => (
                  <tr>
                    <td>
                      <button class="u-link" onClick={() => setModel(m)}>
                        <i class="u-key" style={{ background: color()(m) }} />
                        {m}
                      </button>
                    </td>
                    <td>{costOf(x)}</td>
                    <td>{compact(total(x.tokens))}</td>
                    <td>{cacheHit(x.tokens) === null ? '-' : percent(cacheHit(x.tokens)!)}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </section>
      </div>
      <Show when={model()}>
        {(m) => <ModelDetail model={m()} rows={rows()} days={range()} color={color()(m())} all={s().usd} onClose={() => setModel(null)} />}
      </Show>
    </>
  );
}

function Requests(props: { answer: RequestsAnswer; days: number; provider: string | null; login: string | null; key: string | null }) {
  const key = () => props.key;
  const range = createMemo(() => dates(since(props.days), iso(new Date())));
  const rows = createMemo(() =>
    props.answer.rows.filter(
      (r) =>
        (!props.provider || r.provider === props.provider) &&
        (props.login === null || (r.login ?? '') === props.login) &&
        (key() === null || (r.key ?? '') === key()),
    ),
  );
  const t = createMemo(() => tally(rows()));
  const color = createMemo(() => palette(tallyBy(props.answer.rows, (r) => r.model).map(([m]) => m)));
  const models = createMemo(() => tallyBy(rows(), (r) => r.model));
  const logins = createMemo(() => tallyBy(rows(), (r) => r.login ?? ''));
  const perDay = (value: (r: RequestRow) => number, keyOf: (r: RequestRow) => string) => {
    const index = new Map(range().map((d, i) => [d, i]));
    const out = new Map<string, number[]>();
    for (const r of rows()) {
      const i = index.get(dayOf(r.at));
      if (i === undefined) continue;
      const k = keyOf(r);
      if (!out.has(k)) out.set(k, new Array(range().length).fill(0));
      out.get(k)![i] += value(r);
    }
    return out;
  };
  const trend = createMemo((): Series[] => {
    const per = perDay((r) => total({ input: r.tokens.i ?? 0, output: r.tokens.o ?? 0, cacheWrite: (r.tokens.cw5 ?? 0) + (r.tokens.cw1h ?? 0), cacheRead: r.tokens.cr ?? 0 }), (r) => r.model);
    return models().map(([m]) => ({ id: m, label: m, color: color()(m), values: per.get(m) ?? zeros(range().length) }));
  });
  const activity = createMemo((): Series[] => {
    const per = perDay((r) => r.requests, (r) => r.outcome);
    return [
      { id: 'ok', label: 'Success', color: 'var(--good)', values: per.get('ok') ?? zeros(range().length) },
      { id: 'failed', label: 'Failed', color: 'var(--bad)', values: per.get('failed') ?? zeros(range().length) },
      { id: 'canceled', label: 'Canceled', color: 'var(--muted)', values: per.get('canceled') ?? zeros(range().length) },
    ];
  });
  const minutes = () => props.days * 24 * 60;
  const rate = () => successRate(t());
  const hit = () => cacheHit(t().tokens);
  const speed = () => tps(t());
  const latency = () => averageLatency(t());
  const failing = createMemo(() => models().filter(([, x]) => x.failed > 0).sort((a, b) => b[1].failed - a[1].failed).slice(0, 3));
  return (
    <Show
      when={props.answer.rows.length > 0}
      fallback={
        <p class="note">
          No request through the API proxy in this range. The proxy logs every request it answers;{' '}
          <a href={href({ page: 'proxy' })}>turn it on</a> and point a client at it.
        </p>
      }
    >
      <section class="u-card">
        <div class="u-stats">
          <Stat label="Requests" value={compact(t().requests)} />
          <Stat label="Output tokens/s" value={speed() === null ? '-' : speed()!.toFixed(1)} />
          <Stat label="Tokens" value={compact(total(t().tokens))} />
          <Stat label="Success rate" value={rate() === null ? '-' : percent(rate()!)} note="canceled ones aside" />
          <Stat label="Cache hit" value={hit() === null ? '-' : percent(hit()!)} />
          <Stat label="Average latency" value={latency() === null ? '-' : `${(latency()! / 1000).toFixed(2)} s`} />
          <Stat label="Per minute" value={(t().requests / minutes()).toFixed(2)} />
        </div>
      </section>
      <div class="u-two">
        <section class="u-card">
          <h3>Token trend</h3>
          <StackedBars dates={range()} series={trend()} format={compact} label="Tokens per day per model" />
          <Legend series={trend()} />
        </section>
        <section class="u-card">
          <h3>Token breakdown</h3>
          <SplitBar label="Tokens by type" parts={typeParts(t().tokens, compact)} />
          <p class="page-sub">
            {t().reasoning > 0 ? `${compact(t().reasoning)} of the output was reasoning.` : ''}
          </p>
          <h3>Request outcomes</h3>
          <SplitBar
            label="Request outcomes"
            parts={[
              { label: 'Success', color: 'var(--good)', value: t().ok, shown: compact(t().ok) },
              { label: 'Failed', color: 'var(--bad)', value: t().failed, shown: compact(t().failed) },
              { label: 'Canceled', color: 'var(--muted)', value: t().canceled, shown: compact(t().canceled) },
            ]}
          />
          <Show when={failing().length > 0}>
            <table class="u-table u-gap">
              <thead>
                <tr>
                  <th>Most failures</th>
                  <th>Failed / requests</th>
                </tr>
              </thead>
              <tbody>
                <For each={failing()}>
                  {([m, x]) => (
                    <tr>
                      <td>{m}</td>
                      <td>
                        {x.failed} / {x.requests}
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Show>
        </section>
      </div>
      <section class="u-card">
        <h3>Request activity</h3>
        <StackedBars dates={range()} series={activity()} format={compact} label="Requests per day by outcome" />
        <Legend series={activity()} />
      </section>
      <div class="u-two">
        <section class="u-card">
          <h3>Model consumption</h3>
          <ul class="u-list">
            <For each={models()}>
              {([m, x]) => (
                <li>
                  <i class="u-key" style={{ background: color()(m) }} />
                  <span class="u-name">
                    {m} <small>{x.requests} requests</small>
                  </span>
                  <b>{compact(total(x.tokens))}</b>
                  <small class="u-sub">{total(t().tokens) > 0 ? percent(total(x.tokens) / total(t().tokens)) : '-'} of tokens</small>
                </li>
              )}
            </For>
          </ul>
        </section>
        <section class="u-card">
          <h3>Logins</h3>
          <ul class="u-list">
            <For each={logins()}>
              {([name, x]) => (
                <li>
                  <span class="u-dot" />
                  <span class="u-name">{name || 'Not a stored login'}</span>
                  <b>{compact(total(x.tokens))}</b>
                  <small class="u-sub">{x.requests} requests</small>
                </li>
              )}
            </For>
          </ul>
        </section>
      </div>
    </Show>
  );
}

export function Usage(props: { tab: 'spend' | 'requests' | 'value' }) {
  const [days, setDays] = createSignal(30);
  const [provider, setProvider] = createSignal<string | null>(null);
  const [login, setLogin] = createSignal<string | null>(null);
  const [key, setKey] = createSignal<string | null>(null);
  const [spend, setSpend] = createSignal<SpendAnswer | null>(null);
  const [requests, setRequests] = createSignal<RequestsAnswer | null>(null);
  const [value, setValue] = createSignal<ValueAnswer | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  let fetches = 0;
  createEffect(on(() => props.tab, () => setLogin(null), { defer: true }));
  createEffect(
    on([now, days, () => props.tab], ([, d, tab]) => {
      const seq = ++fetches;
      const done = (e: unknown) => seq === fetches && setError((e as Error).message);
      if (tab === 'spend')
        api<SpendAnswer>('/api/usage/spend', { days: d }).then((r) => seq === fetches && (setSpend(r), setError(null)), done);
      else if (tab === 'value') api<ValueAnswer>('/api/usage/value', {}).then((r) => seq === fetches && (setValue(r), setError(null)), done);
      else
        api<RequestsAnswer>('/api/usage/requests', { days: d }).then((r) => seq === fetches && (setRequests(r), setError(null)), done);
    }),
  );
  const providerIds = createMemo(() => {
    const ids = new Set<string>();
    for (const r of spend()?.export?.rows ?? []) ids.add(r.provider);
    for (const r of requests()?.rows ?? []) ids.add(r.provider);
    for (const w of value()?.weeks ?? []) ids.add(w.provider);
    return [...ids];
  });
  const keys = createMemo(() => [...new Set((requests()?.rows ?? []).map((r) => r.key ?? ''))].sort());
  const loginOptions = createMemo((): [string | null, string][] => {
    const seen = new Map<string, string>();
    if (props.tab === 'spend') {
      for (const r of spend()?.export?.rows ?? []) seen.set(r.login ?? '', loginName(r.login, spend()?.logins));
    } else if (props.tab === 'value') {
      for (const w of value()?.weeks ?? []) if (w.login) seen.set(w.login, w.login);
    } else {
      for (const r of requests()?.rows ?? []) seen.set(r.login ?? '', r.login ?? 'Not a stored login');
    }
    return [[null, 'Every login'], ...[...seen].sort((a, b) => a[1].localeCompare(b[1]))];
  });
  return (
    <div class="page usage">
      <header class="page-head">
        <div>
          <h1>Usage</h1>
          <p class="page-sub">What each login spent, priced by TokenGauge, and what the API proxy answered</p>
        </div>
      </header>
      <nav class="page-tabs" aria-label="Usage">
        <a aria-current={props.tab === 'spend' ? 'page' : undefined} href={href({ page: 'usage', tab: 'spend' })}>
          Spend
        </a>
        <a aria-current={props.tab === 'requests' ? 'page' : undefined} href={href({ page: 'usage', tab: 'requests' })}>
          Proxy requests
        </a>
        <a aria-current={props.tab === 'value' ? 'page' : undefined} href={href({ page: 'usage', tab: 'value' })}>
          Value
        </a>
      </nav>
      <div class="u-filters">
        <Show when={props.tab !== 'value'}>
          <Chips label="Range" value={days()} options={RANGES} onPick={setDays} />
        </Show>
        <Show when={providerIds().length > 1}>
          <Chips
            label="CLI"
            value={provider()}
            options={[[null, 'Every CLI'] as const, ...providerIds().map((id) => [id, nameOf(id)] as const)]}
            onPick={setProvider}
          />
        </Show>
        <Show when={props.tab === 'requests' && keys().length > 1}>
          <Chips
            label="Key"
            value={key()}
            options={[[null, 'Every key'] as const, ...keys().map((k) => [k, k || 'No key'] as const)]}
            onPick={setKey}
          />
        </Show>
        <Show when={loginOptions().length > 2}>
          <label class="u-select">
            <span class="sr">Login</span>
            <select value={login() ?? '*'} onChange={(e) => setLogin(e.currentTarget.value === '*' ? null : e.currentTarget.value)}>
              <For each={loginOptions()}>{([v, text]) => <option value={v ?? '*'}>{text}</option>}</For>
            </select>
          </label>
        </Show>
      </div>
      <Show when={error()}>
        <p class="note">{error()}</p>
      </Show>
      <Show when={props.tab === 'spend'}>
        <Show when={spend()} fallback={<p class="note">Asking TokenGauge…</p>}>
          {(a) => (
            <Show when={a().export} fallback={<p class="note">No spend to show: {a().unavailable}.</p>}>
              <Spend answer={a()} days={days()} provider={provider()} login={login()} />
            </Show>
          )}
        </Show>
      </Show>
      <Show when={props.tab === 'requests'}>
        <Show when={requests()}>{(a) => <Requests answer={a()} days={days()} provider={provider()} login={login()} key={key()} />}</Show>
      </Show>
      <Show when={props.tab === 'value'}>
        <Show when={value()} fallback={<p class="note">Asking TokenGauge…</p>}>
          {(a) => (
            <Show when={a().weeks} fallback={<p class="note">No value to show: {a().unavailable}.</p>}>
              <Value answer={a()} provider={provider()} login={login()} />
            </Show>
          )}
        </Show>
      </Show>
    </div>
  );
}
