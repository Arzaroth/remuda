import { createMemo, createSignal, For, onCleanup, onMount, Show } from 'solid-js';

export type Series = { id: string; label: string; color: string; values: number[] };

const H = 220;
const PAD = { top: 12, right: 12, bottom: 24, left: 56 };

function niceMax(max: number): number {
  if (max <= 0) return 1;
  const step = 10 ** Math.floor(Math.log10(max));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * step >= max) return m * step;
  return 10 * step;
}

const short = (d: string) => new Date(`${d}T12:00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });

function Frame(props: {
  w: number;
  onWidth: (w: number) => void;
  dates: string[];
  max: number;
  format: (v: number) => string;
  label: string;
  hover: number | null;
  onHover: (i: number | null) => void;
  children: any;
  tip: (i: number) => any;
}) {
  const plotW = () => props.w - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  let figure!: HTMLElement;
  onMount(() => {
    const seen = new ResizeObserver(([e]) => props.onWidth(Math.max(240, Math.round(e.contentRect.width))));
    seen.observe(figure);
    onCleanup(() => seen.disconnect());
  });
  const ticks = () => [0, 0.5, 1].map((f) => f * props.max);
  const ends = () => {
    const n = props.dates.length;
    return n <= 1 ? [0] : [0, Math.floor((n - 1) / 2), n - 1];
  };
  const x = (i: number) => PAD.left + (props.dates.length <= 1 ? plotW() / 2 : (i / (props.dates.length - 1)) * plotW());
  let svg!: SVGSVGElement;
  const move = (e: PointerEvent) => {
    const box = svg.getBoundingClientRect();
    const at = ((e.clientX - box.left) / box.width) * props.w;
    const n = props.dates.length;
    if (n === 0) return;
    const i = Math.round(((at - PAD.left) / plotW()) * (n - 1));
    props.onHover(Math.max(0, Math.min(n - 1, i)));
  };
  return (
    <figure class="chart" ref={figure}>
      <svg
        ref={svg}
        viewBox={`0 0 ${props.w} ${H}`}
        height={H}
        role="img"
        aria-label={props.label}
        onPointerMove={move}
        onPointerLeave={() => props.onHover(null)}
      >
        <For each={ticks()}>
          {(t) => {
            const y = PAD.top + plotH - (t / props.max) * plotH;
            return (
              <>
                <line class="grid" x1={PAD.left} x2={props.w - PAD.right} y1={y} y2={y} />
                <text class="axis" x={PAD.left - 8} y={y + 4} text-anchor="end">
                  {props.format(t)}
                </text>
              </>
            );
          }}
        </For>
        <For each={ends()}>
          {(i) => (
            <text class="axis" x={x(i)} y={H - 6} text-anchor={i === 0 ? 'start' : i === props.dates.length - 1 ? 'end' : 'middle'}>
              {props.dates[i] ? short(props.dates[i]) : ''}
            </text>
          )}
        </For>
        {props.children}
        <Show when={props.hover !== null}>
          <line class="cross" x1={x(props.hover!)} x2={x(props.hover!)} y1={PAD.top} y2={PAD.top + plotH} />
        </Show>
      </svg>
      <Show when={props.hover !== null}>
        <div class="chart-tip" style={{ left: `${(x(props.hover!) / props.w) * 100}%` }}>
          <b>{short(props.dates[props.hover!])}</b>
          {props.tip(props.hover!)}
        </div>
      </Show>
    </figure>
  );
}

function TipRows(props: { series: Series[]; i: number; format: (v: number) => string }) {
  return (
    <For each={props.series.filter((s) => s.values[props.i] > 0)}>
      {(s) => (
        <span class="tip-row">
          <i style={{ background: s.color }} />
          {s.label}
          <em>{props.format(s.values[props.i])}</em>
        </span>
      )}
    </For>
  );
}

/** Lines over days, one per series, the area under each tinted. */
export function LineChart(props: { dates: string[]; series: Series[]; format: (v: number) => string; label: string }) {
  const [hover, setHover] = createSignal<number | null>(null);
  const [w, setW] = createSignal(640);
  const max = createMemo(() => niceMax(Math.max(0, ...props.series.flatMap((s) => s.values))));
  const plotH = H - PAD.top - PAD.bottom;
  const point = (i: number, v: number) => {
    const n = props.dates.length;
    const plotW = w() - PAD.left - PAD.right;
    const x = PAD.left + (n <= 1 ? plotW / 2 : (i / (n - 1)) * plotW);
    return `${x.toFixed(1)},${(PAD.top + plotH - (v / max()) * plotH).toFixed(1)}`;
  };
  const line = (s: Series) => s.values.map((v, i) => point(i, v)).join(' ');
  const area = (s: Series) =>
    `${point(0, 0)} ${line(s)} ${point(s.values.length - 1, 0)}`;
  return (
    <Frame
      w={w()}
      onWidth={setW}
      dates={props.dates}
      max={max()}
      format={props.format}
      label={props.label}
      hover={hover()}
      onHover={setHover}
      tip={(i) => <TipRows series={props.series} i={i} format={props.format} />}
    >
      <For each={props.series}>
        {(s) => (
          <>
            <polygon class="area" points={area(s)} style={{ fill: s.color }} />
            <polyline class="line" points={line(s)} style={{ stroke: s.color }} />
          </>
        )}
      </For>
    </Frame>
  );
}

/** One bar per day, its series stacked. */
export function StackedBars(props: { dates: string[]; series: Series[]; format: (v: number) => string; label: string }) {
  const [hover, setHover] = createSignal<number | null>(null);
  const [w, setW] = createSignal(640);
  const totals = createMemo(() => props.dates.map((_, i) => props.series.reduce((a, s) => a + s.values[i], 0)));
  const max = createMemo(() => niceMax(Math.max(0, ...totals())));
  const plotW = () => w() - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const slot = () => plotW() / Math.max(1, props.dates.length - 1 || 1);
  const width = () => Math.max(2, Math.min(18, slot() * 0.6));
  const x = (i: number) => PAD.left + (props.dates.length <= 1 ? plotW() / 2 : (i / (props.dates.length - 1)) * plotW());
  return (
    <Frame
      w={w()}
      onWidth={setW}
      dates={props.dates}
      max={max()}
      format={props.format}
      label={props.label}
      hover={hover()}
      onHover={setHover}
      tip={(i) => <TipRows series={props.series} i={i} format={props.format} />}
    >
      <For each={props.dates}>
        {(_, i) => {
          const parts = () => {
            let base = 0;
            return props.series
              .filter((s) => s.values[i()] > 0)
              .map((s) => {
                const h = (s.values[i()] / max()) * plotH;
                const y = PAD.top + plotH - base - h;
                base += h;
                return { s, y, h };
              });
          };
          return (
            <For each={parts()}>
              {(p) => (
                <rect
                  class="bar"
                  x={x(i()) - width() / 2}
                  y={p.y + 1}
                  width={width()}
                  height={Math.max(0, p.h - 1)}
                  rx={Math.min(2, width() / 2)}
                  style={{ fill: p.s.color }}
                />
              )}
            </For>
          );
        }}
      </For>
    </Frame>
  );
}

export type Part = { label: string; color: string; value: number; shown: string };

/** One bar split by share, with its legend under it. */
export function SplitBar(props: { parts: Part[]; label: string }) {
  const sum = () => props.parts.reduce((a, p) => a + p.value, 0);
  return (
    <div class="split" role="group" aria-label={props.label}>
      <div class="split-bar">
        <For each={props.parts.filter((p) => p.value > 0)}>
          {(p) => <span title={`${p.label}: ${p.shown}`} style={{ 'flex-grow': p.value, background: p.color }} />}
        </For>
        <Show when={sum() === 0}>
          <span class="none" />
        </Show>
      </div>
      <ul class="legend">
        <For each={props.parts}>
          {(p) => (
            <li>
              <i style={{ background: p.color }} />
              {p.label}
              <b>{p.shown}</b>
            </li>
          )}
        </For>
      </ul>
    </div>
  );
}

export function Legend(props: { series: Series[] }) {
  return (
    <ul class="legend">
      <For each={props.series}>
        {(s) => (
          <li>
            <i style={{ background: s.color }} />
            {s.label}
          </li>
        )}
      </For>
    </ul>
  );
}
