import { createMemo, createSignal, For, onCleanup, onMount, Show } from 'solid-js';
import { brand } from '../brand';
import { DAY, HOUR } from '../format';
import { now, store } from '../state';
import { remember, remembered } from '../storage';
import { go } from '../route';
import { burn, capacity, hourAfter, knownFrom, logins, pick, project, rateAt, recorded, remaining, shares, usual, type Login, type Point, type Refill } from '../quota';
import { inUse } from '../windows';
import { weighted, Weighting, windows } from './Strip';
import { Chips } from './Usage';
import { ProviderIcon } from './ProviderIcon';

type View = 'remaining' | 'drain';
type Pace = 'usual' | 1 | 3 | 6;
type Prefs = { view: View; pace: Pace; future: number; windows: Record<string, string> };

const DEFAULTS: Prefs = { view: 'remaining', pace: 'usual', future: 1, windows: {} };
const RECENT = 3;
const SPAN = 14;
const LEAD = 3;

const FUTURES = [
  [1, 'Same as now'],
  [0.5, 'Half'],
  [2, 'Double'],
  [0, 'None'],
] as const;

const pct = (v: number) => `${Math.round(v)}%`;
const days = (d: number) => (d < 1.5 ? 'a day on record' : `${Math.round(d)} days on record`);
const rate = (v: number) => `${v >= 10 ? Math.round(v) : v.toFixed(1)}%/h`;
const factor = (w: number) => `×${w >= 0.995 ? 1 : w.toFixed(2)}`;
const dayFrom = (ms: number, days: number) => {
  const d = new Date(ms);
  d.setDate(d.getDate() + days);
  return d.setHours(0, 0, 0, 0);
};
const shortDay = (ms: number) => new Date(ms).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
const when = (ms: number) => {
  const d = new Date(ms);
  return `${d.toLocaleDateString(undefined, { weekday: 'short' })} ${d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}`;
};

function niceMax(max: number): number {
  if (max <= 0) return 1;
  const step = 10 ** Math.floor(Math.log10(max));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * step >= max) return m * step;
  return 10 * step;
}

const H = 300;
const PAD = { top: 26, right: 16, bottom: 36, left: 52 };

function steps(points: Point[], x: (t: number) => number, y: (v: number) => number): string {
  return points.map(([t, v], i) => `${i ? 'L' : 'M'}${x(t).toFixed(1)} ${y(v).toFixed(1)}`).join(' ');
}

function held(points: Point[]): Point[] {
  return points.flatMap((p, i) => (i ? [[p[0], points[i - 1][1]] as Point, p] : [p]));
}

function valueAt(points: Point[], t: number): number | null {
  let v: number | null = null;
  for (let i = 0; i < points.length; i++) {
    const [at, value] = points[i];
    if (at > t) {
      if (i === 0) return null;
      const [a0, v0] = points[i - 1];
      return at === a0 ? value : v0 + ((value - v0) * (t - a0)) / (at - a0);
    }
    v = value;
  }
  return v;
}

function PoolChart(props: {
  from: number;
  to: number;
  now: number;
  max: number;
  was: Point[];
  ahead: Point[];
  alt?: Point[];
  step: boolean;
  refills: Refill[];
  color: string;
  format: (v: number) => string;
  label: string;
}) {
  const [w, setW] = createSignal(720);
  const [hover, setHover] = createSignal<number | null>(null);
  let figure!: HTMLElement;
  let svg!: SVGSVGElement;
  onMount(() => {
    const seen = new ResizeObserver(([e]) => setW(Math.max(280, Math.round(e.contentRect.width))));
    seen.observe(figure);
    onCleanup(() => seen.disconnect());
  });
  const plotW = () => w() - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const x = (t: number) => PAD.left + ((t - props.from) / (props.to - props.from)) * plotW();
  const y = (v: number) => PAD.top + plotH - (Math.max(0, v) / props.max) * plotH;
  const was = () => (props.step ? held(props.was) : props.was);
  const area = (ps: Point[]) =>
    ps.length > 1 ? `${steps(ps, x, y)} L${x(ps.at(-1)![0]).toFixed(1)} ${y(0)} L${x(ps[0][0]).toFixed(1)} ${y(0)} Z` : '';
  const clip = `pool-${Math.random().toString(36).slice(2)}`;
  const days = () => {
    const out: number[] = [];
    for (let d = dayFrom(props.from, 1); d < props.to; d = dayFrom(d, 1)) out.push(d);
    return out;
  };
  const every = () => Math.max(1, Math.ceil(days().length / Math.max(1, Math.floor(plotW() / 56))));
  const labels = createMemo(() => {
    const placed: { at: number; x: number; gain: number }[] = [];
    for (const r of props.refills) {
      if (r.at < props.from || r.at > props.to || r.gain < 0.5) continue;
      const at = x(r.at);
      if (at - (placed.at(-1)?.x ?? -Infinity) >= 40) placed.push({ at: r.at, x: at, gain: r.gain });
    }
    return placed;
  });
  const top = (at: number) => {
    const v = valueAt([...props.was, ...props.ahead], at);
    return v === null ? PAD.top : y(v);
  };
  const move = (e: PointerEvent) => {
    const box = svg.getBoundingClientRect();
    const at = props.from + (((e.clientX - box.left) / box.width) * w() - PAD.left) / plotW() * (props.to - props.from);
    setHover(at < props.from || at > props.to ? null : at);
  };
  const reading = () => {
    const at = hover();
    if (at === null) return null;
    const past = at <= props.now;
    const v = past ? (props.step ? valueAt(held(props.was), at) : valueAt(props.was, at)) : valueAt(props.ahead, at);
    return v === null ? null : { at, v, past };
  };
  return (
    <figure class="chart pool-chart" ref={figure}>
      <svg
        ref={svg}
        viewBox={`0 0 ${w()} ${H}`}
        height={H}
        role="img"
        aria-label={props.label}
        onPointerMove={move}
        onPointerLeave={() => setHover(null)}
      >
        <defs>
          <clipPath id={clip}>
            <rect x={PAD.left} y={0} width={plotW()} height={PAD.top + plotH} />
          </clipPath>
          <linearGradient id={`${clip}-fill`} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0" stop-color={props.color} stop-opacity="0.22" />
            <stop offset="1" stop-color={props.color} stop-opacity="0.02" />
          </linearGradient>
        </defs>
        <For each={[0, 0.5, 1]}>
          {(f) => (
            <>
              <line class="grid" x1={PAD.left} x2={w() - PAD.right} y1={y(f * props.max)} y2={y(f * props.max)} />
              <text class="axis" x={PAD.left - 8} y={y(f * props.max) + 4} text-anchor="end">
                {props.format(f * props.max)}
              </text>
            </>
          )}
        </For>
        <For each={days()}>
          {(d, i) => (
            <>
              <line class="grid faint" x1={x(d)} x2={x(d)} y1={PAD.top} y2={PAD.top + plotH} />
              <Show when={i() % every() === 0}>
                <text class="axis" x={x(d + DAY / 2)} y={H - 20} text-anchor="middle">
                  {new Date(d).toLocaleDateString(undefined, { weekday: 'short' })}
                </text>
                <text class="axis" x={x(d + DAY / 2)} y={H - 6} text-anchor="middle">
                  {new Date(d).getDate()}
                </text>
              </Show>
            </>
          )}
        </For>
        <g clip-path={`url(#${clip})`}>
          <path class="pool-area" d={area(was())} style={{ fill: `url(#${clip}-fill)` }} />
          <path class="pool-area ahead" d={area(props.ahead)} style={{ fill: `url(#${clip}-fill)` }} />
          <path class="line" d={steps(was(), x, y)} style={{ stroke: props.color }} />
          <path class="line ahead" d={steps(props.ahead, x, y)} style={{ stroke: props.color }} />
          <Show when={props.alt?.length}>
            <path class="line alt" d={steps(props.alt!, x, y)} style={{ stroke: props.color }} />
          </Show>
        </g>
        <Show when={props.now >= props.from && props.now <= props.to}>
          <line class="now" x1={x(props.now)} x2={x(props.now)} y1={PAD.top - 8} y2={PAD.top + plotH} />
          <text class="axis" x={x(props.now)} y={PAD.top - 12} text-anchor="middle">
            Now
          </text>
          <Show when={props.ahead.length}>
            <circle class="now-dot" cx={x(props.now)} cy={y(props.ahead[0][1])} r="4" style={{ fill: props.color }} />
          </Show>
        </Show>
        <For each={labels()}>
          {(l) => (
            <text class="refill" x={l.x} y={top(l.at) - 8} text-anchor="middle">
              +{pct(l.gain)}
            </text>
          )}
        </For>
        <Show when={reading()}>
          {(r) => <line class="cross" x1={x(r().at)} x2={x(r().at)} y1={PAD.top} y2={PAD.top + plotH} />}
        </Show>
      </svg>
      <Show when={reading()}>
        {(r) => (
          <div class="chart-tip" style={{ left: `${(x(r().at) / w()) * 100}%` }}>
            <b>{when(r().at)}</b>
            <span class="tip-row">
              {r().past ? 'Recorded' : 'Projected'}
              <em>{props.format(r().v)}</em>
            </span>
          </div>
        )}
      </Show>
    </figure>
  );
}

function Fig(props: { label: string; value: string; note: string; lead?: boolean }) {
  return (
    <div class="pool-fig" classList={{ lead: props.lead }}>
      <dt>{props.label}</dt>
      <dd class="fig-value" title={props.value}>
        {props.value}
      </dd>
      <dd class="fig-note">
        {props.note}
      </dd>
    </div>
  );
}

function Pool(props: { id: string; name: string; prefs: Prefs; offset: number; setPrefs: (p: Partial<Prefs>) => void }) {
  const all = () => windows(props.id);
  const c = createMemo(() => pick(all(), props.prefs.windows[props.id]));
  const t = () => now();
  const ls = createMemo((): Login[] => (c() ? logins(c()!, t(), store.s?.usage?.updatedAt ?? null) : []));
  const from = () => dayFrom(t(), props.offset * 7 - LEAD);
  const to = () => dayFrom(from(), SPAN);
  const isUsual = () => props.prefs.pace === 'usual';
  const hours = () => (props.prefs.pace === 'usual' ? RECENT : props.prefs.pace);
  const spent = () => burn(ls(), t(), hours());
  const week = createMemo(() => usual(ls(), t()));
  const horizon = () => Math.max(to(), t() + 7 * DAY);
  const pace = createMemo(() => {
    const f = props.prefs.future;
    if (!isUsual()) return spent() === null ? null : spent()! * f;
    const w = week();
    if (!w) return null;
    const at = rateAt(w.slots);
    return (time: number) => at(time) * f;
  });
  const ahead = createMemo(() => project(ls(), t(), pace() ?? 0, horizon()));
  const alt = createMemo(() => (isUsual() && spent() !== null ? project(ls(), t(), spent()! * props.prefs.future, horizon()).points : []));
  const paceAt = (time: number) => {
    const p = pace();
    return p === null ? 0 : typeof p === 'number' ? p : p(time);
  };
  const drainAhead = createMemo((): Point[] => {
    if (pace() === null) return [];
    const out: Point[] = [];
    for (let at = t(); at < Math.max(to(), t()); at = hourAfter(at)) {
      const next = Math.min(Math.max(to(), t()), hourAfter(at));
      out.push([at, paceAt(at)], [next, paceAt(at)]);
    }
    return out;
  });
  const was = createMemo(() => recorded(ls()));
  const drain = createMemo((): Point[] => {
    if (!ls().length) return [];
    const out: Point[] = [];
    for (let at = Math.max(knownFrom(ls()), from()); at <= t(); at += HOUR) {
      const r = burn(ls(), at, hours());
      if (r !== null) out.push([at, r]);
    }
    const last = burn(ls(), t(), hours());
    if (last !== null) out.push([t(), last]);
    return out;
  });
  const next = () => ahead().refills.find((r) => r.at > t());
  const each = createMemo(() => {
    const all = shares(ls(), t(), hours(), isUsual() ? week() : null);
    return isUsual() && !week() ? all.map((s) => ({ ...s, own: null, pooled: null, empty: null })) : all;
  });
  const noPace = () => (isUsual() ? 'under a day on record' : 'no burn yet');
  const cap = () => capacity(ls());
  return (
    <section class="u-card pool" style={{ '--brand': brand(props.id) }}>
      <header class="pool-head">
        <h3 class="u-plan">
          <ProviderIcon provider={props.id} size={20} />
          <span>{props.name}</span>
          <small class="u-sub">
            {ls().length} {ls().length === 1 ? 'login' : 'logins'}
            <Show when={c() && all().length < 2}> · {c()!.title}</Show>
          </small>
        </h3>
        <div class="pool-tools">
          <Weighting id={props.id} />
          <Show when={c() && weighted(props.id) && !c()!.weighted}>
            <small class="u-sub" title="TokenGauge has no plan weights for this window, so its figures count each login once">
              each login once here
            </small>
          </Show>
          <Show when={all().length > 1}>
            <label class="u-select">
              <span class="sr">Window</span>
              <select
                value={c()?.title}
                onChange={(e) => props.setPrefs({ windows: { ...props.prefs.windows, [props.id]: e.currentTarget.value } })}
              >
                <For each={all()}>
                  {(w) => (
                    <option value={w.title} selected={w.title === c()?.title}>
                      {w.title}
                    </option>
                  )}
                </For>
              </select>
            </label>
          </Show>
        </div>
      </header>
      <Show when={c()} fallback={<p class="note">TokenGauge has no figures for this CLI's logins yet.</p>}>
        <dl class="pool-top">
          <Fig label="Remaining" value={pct(remaining(ls()))} note={`of ${pct(cap())}`} lead />
          <Fig
            label="Usual week"
            value={week() ? pct(week()!.week) : '-'}
            note={week() ? `needed, of ${pct(cap())}, from ${days(week()!.days)}` : 'under a day on record'}
          />
          <Fig
            label="Pool empty"
            value={ahead().poolEmpty ? when(ahead().poolEmpty!) : pace() === null ? '-' : 'Never'}
            note={ahead().poolEmpty ? (isUsual() ? 'in a usual week' : 'at this pace') : pace() === null ? noPace() : `through ${shortDay(horizon())}`}
          />
          <Fig
            label="First empty"
            value={pace() === null ? '-' : ahead().firstEmpty ? ahead().firstEmpty!.name : 'None'}
            note={pace() === null ? noPace() : ahead().firstEmpty ? when(ahead().firstEmpty!.at) : 'before a reset'}
          />
          <Fig
            label="Next reset"
            value={next() ? `+${pct(next()!.gain)}` : 'None'}
            note={next() ? `${next()!.name}, ${when(next()!.at)}` : 'ahead'}
          />
          <Fig
            label="Burn"
            value={spent() === null ? '-' : rate(spent()!)}
            note={spent() === null ? 'not recorded yet' : `over the last ${hours()}h`}
          />
        </dl>
        <div class="pool-logins">
          <table class="u-table">
            <thead>
              <tr>
                <th>Login</th>
                <th>Left</th>
                <th>{isUsual() ? 'Usual burn' : 'Burn'}</th>
                <Show when={c()!.weighted}>
                  <th title="In units of the largest plan, so the logins add up to the Burn figure">Of the pool</th>
                </Show>
                <th>{isUsual() ? 'Empty in a usual week' : 'Empty at its pace'}</th>
                <th>Resets</th>
              </tr>
            </thead>
            <tbody>
              <For each={each()}>
                {(l) => (
                  <tr>
                    <td>
                      {l.name}
                      <Show when={c()!.weighted}>
                        <small class="u-sub"> {factor(l.weight)}</small>
                      </Show>
                    </td>
                    <td>{pct(l.left)}</td>
                    <td>{l.own === null ? '-' : rate(l.own)}</td>
                    <Show when={c()!.weighted}>
                      <td>{l.pooled === null ? '-' : rate(l.pooled)}</td>
                    </Show>
                    <td>{l.empty ? when(l.empty) : l.own === null ? '-' : l.own ? 'Resets first' : 'Not burning'}</td>
                    <td>{l.resetsAt ? when(l.resetsAt) : '-'}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
        <Show
          when={props.prefs.view === 'remaining'}
          fallback={
            <PoolChart
              from={from()}
              to={to()}
              now={t()}
              max={niceMax(Math.max(0.1, ...drain().map(([, v]) => v), ...drainAhead().map(([, v]) => v)))}
              was={drain()}
              ahead={drainAhead()}
              step={false}
              refills={[]}
              color={brand(props.id)}
              format={rate}
              label={`${props.name}'s pooled burn rate, recorded and projected`}
            />
          }
        >
          <PoolChart
            from={from()}
            to={to()}
            now={t()}
            max={cap()}
            was={was().points}
            ahead={pace() === null ? [] : ahead().points}
            alt={alt()}
            step
            refills={[...was().refills, ...ahead().refills]}
            color={brand(props.id)}
            format={pct}
            label={`${props.name}'s pooled remaining quota, recorded and projected`}
          />
        </Show>
        <ul class="legend pool-legend">
          <li>
            <i class="solid" />
            Recorded
          </li>
          <li>
            <i class="dashed" />
            {isUsual() ? 'Projected, a usual week' : 'Projected'}
          </li>
          <Show when={props.prefs.view === 'remaining' && alt().length}>
            <li>
              <i class="dotted" />
              At the last {RECENT}h's pace
            </li>
          </Show>
          <Show when={c()!.leftOut.length}>
            <li class="u-sub">Left out, no known plan weight: {c()!.leftOut.join(', ')}</li>
          </Show>
        </ul>
      </Show>
    </section>
  );
}

export function Quota(props: { id: string | null }) {
  const [prefs, setPrefsSignal] = createSignal<Prefs>({ ...DEFAULTS, ...(remembered('remuda-quota') || {}) });
  const setPrefs = (p: Partial<Prefs>) => {
    setPrefsSignal({ ...prefs(), ...p });
    remember('remuda-quota', prefs());
  };
  const [offset, setOffset] = createSignal(0);
  const providers = () => inUse(store.s!).filter((p) => windows(p.id).length);
  const shown = () => (props.id ? providers().filter((p) => p.id === props.id) : providers());
  const from = () => dayFrom(now(), offset() * 7 - LEAD);
  return (
    <div class="page">
      <header class="page-head">
        <div>
          <h1>Quota</h1>
          <p class="page-sub">
            Every login of a CLI pooled into one allowance: what is left, and where it is heading if the coming days go the way your
            weeks usually do, hour by hour, or if the recent burn holds. Each reset fills its login back up, and the projection spends
            the login that resets soonest first.
          </p>
        </div>
      </header>
      <div class="u-filters">
        <Show when={providers().length > 1}>
          <div class="chips" role="group" aria-label="CLI">
            <button type="button" class="chip" aria-pressed={!props.id} onClick={() => go({ page: 'quota', id: null })}>
              All
            </button>
            <For each={providers()}>
              {(p) => (
                <button type="button" class="chip" aria-pressed={props.id === p.id} onClick={() => go({ page: 'quota', id: p.id })}>
                  <ProviderIcon provider={p.id} size={16} />
                  {p.name}
                </button>
              )}
            </For>
          </div>
        </Show>
      </div>
      <div class="u-filters">
        <Chips
          label="View"
          value={prefs().view}
          options={[
            ['remaining', 'Remaining'],
            ['drain', 'Drain rate'],
          ]}
          onPick={(view) => setPrefs({ view })}
        />
        <span class="u-label">Pace</span>
        <Chips<Pace>
          label="Pace"
          value={prefs().pace}
          options={[
            ['usual', 'Usual week'],
            [1, 'Last 1h'],
            [3, 'Last 3h'],
            [6, 'Last 6h'],
          ]}
          onPick={(pace) => setPrefs({ pace })}
        />
        <label class="u-select">
          <span class="u-label">Future usage</span>
          <select value={prefs().future} onChange={(e) => setPrefs({ future: Number(e.currentTarget.value) })}>
            <For each={FUTURES}>{([v, text]) => <option value={v}>{text}</option>}</For>
          </select>
        </label>
        <div class="pool-range">
          <span>
            {shortDay(from())} to {shortDay(dayFrom(from(), SPAN - 1))}
          </span>
          <button class="btn icon-btn" aria-label="Earlier" disabled={offset() <= -2} onClick={() => setOffset(offset() - 1)}>
            ‹
          </button>
          <button class="btn" disabled={offset() === 0} onClick={() => setOffset(0)}>
            Today
          </button>
          <button class="btn icon-btn" aria-label="Later" disabled={offset() >= 4} onClick={() => setOffset(offset() + 1)}>
            ›
          </button>
        </div>
      </div>
      <Show when={shown().length} fallback={<p class="note">TokenGauge has no usage figures for any CLI yet.</p>}>
        <For each={shown()}>{(p) => <Pool id={p.id} name={p.name} prefs={prefs()} offset={offset()} setPrefs={setPrefs} />}</For>
      </Show>
    </div>
  );
}
