import { createMemo, createSignal, For, onCleanup, onMount, Show } from 'solid-js';
import { brand } from '../brand';
import { DAY, HOUR } from '../format';
import { now, store } from '../state';
import { remember, remembered } from '../storage';
import { go } from '../route';
import { burn, capacity, knownFrom, logins, pick, project, recorded, remaining, type Login, type Point, type Refill } from '../quota';
import { inUse } from '../windows';
import { windows } from './Strip';
import { Chips } from './Usage';
import { ProviderIcon } from './ProviderIcon';

type View = 'remaining' | 'drain';
type Prefs = { view: View; hours: number; future: number; windows: Record<string, string> };

const DEFAULTS: Prefs = { view: 'remaining', hours: 3, future: 1, windows: {} };
const SPAN = 14 * DAY;
const LEAD = 3 * DAY;

const FUTURES = [
  [1, 'Same as now'],
  [0.5, 'Half'],
  [2, 'Double'],
  [0, 'None'],
] as const;

const pct = (v: number) => `${Math.round(v)}%`;
const rate = (v: number) => `${v >= 10 ? Math.round(v) : v.toFixed(1)}%/h`;
const midnight = (ms: number) => new Date(ms).setHours(0, 0, 0, 0);
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
    const d = new Date(midnight(props.from));
    for (d.setDate(d.getDate() + 1); d.getTime() < props.to; d.setDate(d.getDate() + 1)) out.push(d.getTime());
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
  const from = () => midnight(t() - LEAD) + props.offset * 7 * DAY;
  const to = () => from() + SPAN;
  const spent = () => burn(ls(), t(), props.prefs.hours);
  const horizon = () => Math.max(to(), t() + 7 * DAY);
  const ahead = createMemo(() => project(ls(), t(), (spent() ?? 0) * props.prefs.future, horizon()));
  const was = createMemo(() => recorded(ls()));
  const drain = createMemo((): Point[] => {
    if (!ls().length) return [];
    const out: Point[] = [];
    for (let at = Math.max(knownFrom(ls()), from()); at <= t(); at += HOUR) {
      const r = burn(ls(), at, props.prefs.hours);
      if (r !== null) out.push([at, r]);
    }
    const last = burn(ls(), t(), props.prefs.hours);
    if (last !== null) out.push([t(), last]);
    return out;
  });
  const next = () => ahead().refills.find((r) => r.at > t());
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
        <Show when={all().length > 1}>
          <label class="u-select">
            <span class="sr">Window</span>
            <select
              value={c()?.title}
              onChange={(e) => props.setPrefs({ windows: { ...props.prefs.windows, [props.id]: e.currentTarget.value } })}
            >
              <For each={all()}>{(w) => <option value={w.title}>{w.title}</option>}</For>
            </select>
          </label>
        </Show>
      </header>
      <Show when={c()} fallback={<p class="note">TokenGauge has no figures for this CLI's logins yet.</p>}>
        <dl class="pool-top">
          <Fig label="Remaining" value={pct(remaining(ls()))} note={`of ${pct(cap())}`} lead />
          <Fig
            label="Pool empty"
            value={ahead().poolEmpty ? when(ahead().poolEmpty!) : spent() === null ? '-' : 'Never'}
            note={ahead().poolEmpty ? 'at this pace' : spent() === null ? 'no burn yet' : `through ${shortDay(horizon())}`}
          />
          <Fig
            label="First empty"
            value={spent() === null ? '-' : ahead().firstEmpty ? ahead().firstEmpty!.name : 'None'}
            note={spent() === null ? 'no burn yet' : ahead().firstEmpty ? when(ahead().firstEmpty!.at) : 'before a reset'}
          />
          <Fig
            label="Next reset"
            value={next() ? `+${pct(next()!.gain)}` : 'None'}
            note={next() ? `${next()!.name}, ${when(next()!.at)}` : 'ahead'}
          />
          <Fig
            label="Burn"
            value={spent() === null ? '-' : rate(spent()!)}
            note={spent() === null ? 'not recorded yet' : `over the last ${props.prefs.hours}h`}
          />
        </dl>
        <Show
          when={props.prefs.view === 'remaining'}
          fallback={
            <PoolChart
              from={from()}
              to={to()}
              now={t()}
              max={niceMax(Math.max(0.1, ...drain().map(([, v]) => v), (spent() ?? 0) * props.prefs.future))}
              was={drain()}
              ahead={spent() === null ? [] : [[t(), spent()! * props.prefs.future], [Math.max(to(), t()), spent()! * props.prefs.future]]}
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
            ahead={ahead().points}
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
            Projected
          </li>
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
  const from = () => midnight(now() - LEAD) + offset() * 7 * DAY;
  return (
    <div class="page">
      <header class="page-head">
        <div>
          <h1>Quota</h1>
          <p class="page-sub">
            Every login of a CLI pooled into one allowance: what is left, and where it is heading if the recent burn holds and each
            reset fills its login back up. The projection spends the login that resets soonest first.
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
        <span class="u-label">Burn rate</span>
        <Chips
          label="Burn rate over"
          value={prefs().hours}
          options={[
            [1, '1h'],
            [3, '3h'],
            [6, '6h'],
          ]}
          onPick={(hours) => setPrefs({ hours })}
        />
        <label class="u-select">
          <span class="u-label">Future usage</span>
          <select value={prefs().future} onChange={(e) => setPrefs({ future: Number(e.currentTarget.value) })}>
            <For each={FUTURES}>{([v, text]) => <option value={v}>{text}</option>}</For>
          </select>
        </label>
        <div class="pool-range">
          <span>
            {shortDay(from())} to {shortDay(from() + SPAN - DAY)}
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
