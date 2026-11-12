import { createMemo, For, Show } from 'solid-js';
import { brand } from '../brand';
import { money } from '../spend';
import { store } from '../state';
import { change, changeText, cutShort, plans, weights, type ValueAnswer, type Week } from '../value';
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
        <For each={shown()}>
          {(p) => {
            const ratio = () => change(p.weeks);
            const charted = () => p.weeks.filter((w) => !w.open && !w.partial && !w.unpriced && w.usdPerPercent != null);
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
                    <For each={[...p.weeks].reverse()}>
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
              </section>
            );
          }}
        </For>
      </Show>
    </>
  );
}
