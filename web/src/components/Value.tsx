import { createMemo, createSignal, For, Show } from 'solid-js';
import { brand } from '../brand';
import { money } from '../spend';
import { now, store } from '../state';
import { remember, remembered } from '../storage';
import {
  arrange,
  change,
  changed,
  changeText,
  cutShort,
  FILTERS,
  plans,
  weights,
  type Filters,
  type Order,
  type Plan,
  type ValueAnswer,
  type Week,
} from '../value';
import { LineChart } from './Charts';
import { ProviderIcon } from './ProviderIcon';

const iso = (ms: number) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
};
const factor = (w: number) => `×${w >= 0.995 && w <= 1.005 ? 1 : w.toFixed(2)}`;
const day = (ms: number) => new Date(ms).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });

function flags(w: Week): string {
  const out: string[] = [];
  if (w.open) out.push('so far');
  const days = cutShort(w);
  if (days != null) out.push(days < 1 ? 'reset the same day' : `reset after ${Math.round(days)} day${Math.round(days) === 1 ? '' : 's'}`);
  if (w.partial) out.push('the record starts mid-week');
  if (w.unpriced) out.push('some use has no price');
  if (w.unattributedUsd >= 0.01) out.push(`${money(w.unattributedUsd)} more no login claims`);
  return out.join(' · ');
}

const PERIODS: [number, string][] = [
  [1, '1 month'],
  [3, '3 months'],
  [6, '6 months'],
  [12, '1 year'],
  [0, 'All'],
];
const HIDE: [keyof Filters, string][] = [
  ['removed', 'Removed logins'],
  ['unclaimed', 'Weeks with unclaimed spend'],
  ['open', 'The week in progress'],
  ['incomplete', 'Incomplete weeks'],
  ['idle', 'Weeks with no spend'],
  ['cut', 'Weeks cut short by a reset'],
];
const ORDERS: [Order, string][] = [
  ['recent', 'Latest week first'],
  ['value', 'Best value first'],
  ['name', 'By name'],
];
const weeksText = (n: number) => `${n} week${n === 1 ? '' : 's'}`;
const names = (ps: Plan[]) => ps.map((p) => p.login).join(', ');

export function Value(props: { answer: ValueAnswer; provider: string | null; login: string | null }) {
  const shown = createMemo(() =>
    plans(props.answer.weeks ?? []).filter(
      (p) => (!props.provider || p.provider === props.provider) && (!props.login || p.login === props.login),
    ),
  );
  const advertised = createMemo(() => {
    const out: Record<string, Record<string, number>> = {};
    for (const [id, shown] of Object.entries(store.s?.shown ?? {})) {
      const parts = shown.weighted.filter((c) => c.weighted).flatMap((c) => c.parts);
      if (parts.length) out[id] = Object.fromEntries(parts.map((p) => [p.name, p.weight]));
    }
    return out;
  });
  const measured = createMemo(() => weights(plans(props.answer.weeks ?? []), advertised()));
  const [filters, setFilters] = createSignal<Filters>({ ...FILTERS, ...(remembered<Partial<Filters>>('remuda-value-filters') || {}) });
  const set = (patch: Partial<Filters>) => {
    const next = { ...filters(), ...patch };
    setFilters(next);
    remember('remuda-value-filters', next);
  };
  const view = createMemo(() => arrange(shown(), filters(), props.answer.logins, now()));
  const hidden = () =>
    [...view().shown.map((s) => s.hidden), ...[...view().empty, ...view().gone].map((p) => p.weeks.length)].reduce((a, n) => a + n, 0);
  const active = () => changed(filters());
  return (
    <>
      <p class="page-sub">
        What a week of each login bought, in API dollars per percent of its weekly allowance. It counts only what TokenGauge saw on the
        machines it syncs: use on the web, in the desktop apps or on a machine without it makes a week look worth less than it was.
      </p>
      <Show
        when={shown().length}
        fallback={
          <p class="note">
            {props.answer.weeks?.length
              ? 'No week recorded for this CLI or login.'
              : 'No week has been recorded yet: TokenGauge 0.56 or later keeps them once they close.'}
          </p>
        }
      >
        <details class="v-filters">
          <summary class="chip" aria-pressed={active() > 0}>
            Filters{active() ? ` · ${active()}` : ''}
            <Show when={hidden()}> · {weeksText(hidden())} hidden</Show>
          </summary>
          <div class="v-panel">
            <div class="v-row">
              <div class="chips" role="group" aria-label="Period">
                <For each={PERIODS}>
                  {([months, text]) => (
                    <button type="button" class="chip" aria-pressed={filters().months === months} onClick={() => set({ months })}>
                      {text}
                    </button>
                  )}
                </For>
              </div>
              <label class="v-range">
                <span>Used at least</span>
                <input
                  type="range"
                  min="0"
                  max="100"
                  step="5"
                  value={filters().minUsed}
                  onInput={(e) => set({ minUsed: Number(e.currentTarget.value) })}
                />
                <output>{filters().minUsed}%</output>
              </label>
            </div>
            <div class="chips" role="group" aria-label="Hide">
              <span class="v-label">Hide</span>
              <For each={HIDE}>
                {([key, text]) => (
                  <button type="button" class="chip" aria-pressed={!!filters()[key]} onClick={() => set({ [key]: !filters()[key] })}>
                    {text}
                  </button>
                )}
              </For>
            </div>
            <div class="chips" role="group" aria-label="Order">
              <span class="v-label">Order</span>
              <For each={ORDERS}>
                {([order, text]) => (
                  <button type="button" class="chip" aria-pressed={filters().order === order} onClick={() => set({ order })}>
                    {text}
                  </button>
                )}
              </For>
            </div>
            <Show when={active()}>
              <button type="button" class="u-link v-reset" onClick={() => set({ ...FILTERS, order: filters().order })}>
                Reset filters
              </button>
            </Show>
          </div>
        </details>
        <Show when={!view().shown.length}>
          <p class="note">No week left with these filters.</p>
        </Show>
        <For each={view().shown}>
          {({ plan: p, weeks, hidden }) => {
            const ratio = () => change(p.weeks);
            const charted = () => weeks.filter((w) => !w.open && !w.partial && !w.unpriced && w.usdPerPercent != null);
            return (
              <section class="u-card">
                <h3 class="u-plan">
                  <ProviderIcon provider={p.provider} size={20} />
                  <span>{p.login}</span>
                  <Show when={ratio() != null}>
                    <small class="u-sub">Last week: {changeText(ratio()!)}</small>
                  </Show>
                  <Show when={measured().get(p)}>
                    {(w) => (
                      <small
                        class="u-sub"
                        title="The plan weight TokenGauge gives this login, and the one its dollars per percent over its last four whole weeks say, against the largest plan's"
                      >
                        Weight {factor(w().advertised)} advertised, {factor(w().measured)} measured
                      </small>
                    )}
                  </Show>
                </h3>
                <Show when={charted().length > 1}>
                  <LineChart
                    dates={charted().map((w) => iso(w.openedAt))}
                    series={[{ id: p.login, label: 'Per 1%', color: brand(p.provider), values: charted().map((w) => w.usdPerPercent!) }]}
                    format={money}
                    label={`API dollars per percent of ${p.login}'s week`}
                  />
                </Show>
                <table class="u-table">
                  <thead>
                    <tr>
                      <th>Week</th>
                      <th>Used</th>
                      <th>Spent</th>
                      <th>Per 1%</th>
                      <th />
                    </tr>
                  </thead>
                  <tbody>
                    <For each={[...weeks].reverse()}>
                      {(w) => (
                        <tr>
                          <td>
                            {day(w.openedAt)} to {w.open ? 'now' : day(w.endsAt)}
                          </td>
                          <td>{w.used}%</td>
                          <td>{money(w.usd)}</td>
                          <td>{w.usdPerPercent == null ? '-' : money(w.usdPerPercent)}</td>
                          <td class="u-note">{flags(w)}</td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
                <Show when={hidden}>
                  <p class="v-hidden">{weeksText(hidden)} hidden by the filters</p>
                </Show>
              </section>
            );
          }}
        </For>
        <Show when={view().empty.length || view().gone.length}>
          <p class="v-hidden">
            <Show when={view().empty.length}>No week left to show for {names(view().empty)}. </Show>
            <Show when={view().gone.length}>
              Removed login{view().gone.length === 1 ? '' : 's'} hidden: {names(view().gone)}.
            </Show>
          </p>
        </Show>
      </Show>
    </>
  );
}
