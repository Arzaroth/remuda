import { createMemo, For, Show } from 'solid-js';
import { createStore } from 'solid-js/store';
import { until, usedTone } from '../format';
import { now, store } from '../state';
import { remember, remembered } from '../storage';
import type { Provider } from '../types';
import { providerUsage, usageOf } from '../usage';
import { Meter } from './Meter';

const [picks, setPicks] = createStore<Record<string, string>>(remembered('remuda-tiles') || {});

export function Strip() {
  return (
    <div class="strip">
      <For each={store.s!.providers}>{(p) => <Tile p={p} />}</For>
    </div>
  );
}

function Tile(props: { p: Provider }) {
  const s = () => store.s!;
  const creds = () => s().credentials.filter((c) => c.provider === props.p.id);
  const active = () => creds().find((c) => c.active);
  const live = () => s().live.find((l) => l.provider === props.p.id);
  const usage = createMemo(() => {
    const u = providerUsage(s(), props.p.id);
    const a = active();
    return a ? usageOf(a, u) : u;
  });
  const windows = () => {
    const u = usage();
    return u && !u.error ? u.windows : [];
  };
  const shown = () =>
    windows().find((w) => w.title === picks[props.p.id]) ||
    windows().reduce((a, b) => (b.usedPercent > a.usedPercent ? b : a));
  const pick = (title: string) => {
    setPicks(props.p.id, title);
    remember('remuda-tiles', { ...picks });
  };
  return (
    <div class="tile">
      <div class="tile-head">
        <b>{props.p.name}</b>
        <span>
          {creds().length} login{creds().length === 1 ? '' : 's'}
        </span>
      </div>
      <Show
        when={windows().length}
        fallback={
          <div class="big">
            &ndash;<small>{usage()?.behind ? 'waiting for TokenGauge' : 'no usage figures'}</small>
          </div>
        }
      >
        <div class="big">
          {shown().usedPercent}%<small>used, {shown().title} limit</small>
        </div>
        <Meter fraction={shown().usedPercent / 100} tone={usedTone(shown().usedPercent)} />
      </Show>
      <div class="under">
        <Show
          when={active()}
          fallback={
            live()?.state === 'unstored'
              ? 'A login remuda has not stored is in use'
              : live()?.state === 'signed_out'
                ? 'Signed out'
                : 'No stored login in use'
          }
        >
          <span class="spec">{active()!.name}</span> in use
        </Show>
        <Show when={windows().length && shown().resetsAt}>, resets {until(Date.parse(shown().resetsAt!), now())}</Show>
      </div>
      <Show when={windows().length > 1}>
        <div class="picks" role="group" aria-label="Limit shown">
          <For each={windows()}>
            {(w) => (
              <button aria-pressed={w === shown()} title={`${w.usedPercent}% used`} onClick={() => pick(w.title)}>
                {w.title}
              </button>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
