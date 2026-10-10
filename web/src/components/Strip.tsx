import { createMemo, For, Show } from 'solid-js';
import { createStore } from 'solid-js/store';
import { brand } from '../brand';
import { date, plural, until, usedTone } from '../format';
import { now, store } from '../state';
import { remember, remembered } from '../storage';
import type { Provider } from '../types';
import { combine, inUse, providerUsage, weighable } from '../usage';
import { Meter } from './Meter';

const [picks, setPicks] = createStore<Record<string, string>>(remembered('remuda-tiles') || {});
const [modes, setModes] = createStore<Record<string, 'weighted' | 'absolute'>>(remembered('remuda-weighting') || {});

const ESTIMATE =
  "Weighted by each plan's nominal multiplier, in units of the largest plan, as TokenGauge does. The real limits are not published, so this is an estimate.";

export function Strip() {
  return (
    <div class="strip">
      <For each={inUse(store.s!)}>{(p) => <Tile p={p} />}</For>
    </div>
  );
}

function Tile(props: { p: Provider }) {
  const s = () => store.s!;
  const creds = () => s().credentials.filter((c) => c.provider === props.p.id);
  const active = () => creds().find((c) => c.active);
  const live = () => s().live.find((l) => l.provider === props.p.id);
  const usage = () => providerUsage(s(), props.p.id);
  const canWeigh = () => weighable(creds(), usage());
  const weighted = () => canWeigh() && modes[props.p.id] !== 'absolute';
  const setMode = (mode: 'weighted' | 'absolute') => {
    setModes(props.p.id, mode);
    remember('remuda-weighting', { ...modes });
  };
  const combined = createMemo(() => combine(creds(), usage(), weighted()));
  const shown = () =>
    combined().find((w) => w.title === picks[props.p.id]) ||
    combined().reduce((a, b) => (b.pooled > a.pooled ? b : a));
  const others = () => combined().filter((w) => w !== shown());
  const pick = (title: string) => {
    setPicks(props.p.id, title);
    remember('remuda-tiles', { ...picks });
  };
  return (
    <div class="tile" style={{ '--brand': brand(props.p.id) }}>
      <div class="tile-head">
        <b>{props.p.name}</b>
        <span>{plural(creds().length, 'login')}</span>
      </div>
      <Show
        when={combined().length}
        fallback={
          <div class="big">
            &ndash;<small>{usage()?.behind ? 'waiting for TokenGauge' : 'no usage figures'}</small>
          </div>
        }
      >
        <div class="tile-window">
          <span>{shown().title}</span>
          <Show when={canWeigh()}>
            <div class="weighting" role="group" aria-label="How logins add up">
              <button
                aria-pressed={weighted()}
                title="Each login counts by its plan's multiplier"
                onClick={() => setMode('weighted')}
              >
                Weighted
              </button>
              <button aria-pressed={!weighted()} title="Each login counts once" onClick={() => setMode('absolute')}>
                Absolute
              </button>
            </div>
          </Show>
        </div>
        <div class="big" title={shown().weighted ? ESTIMATE : undefined}>
          {shown().used}%<small>of {shown().of}%</small>
        </div>
        <div
          class="segments"
          role="img"
          aria-label={shown()
            .parts.map((p) => `${p.name} ${p.window.usedPercent}%`)
            .join(', ')}
        >
          <For each={shown().parts}>
            {(p, i) => (
              <div
                class="segment"
                classList={{ active: p.active }}
                style={{ 'flex-grow': shown().widths[i()] }}
                title={`${p.name}${p.active ? ' (in use)' : ''}: ${p.window.usedPercent}%${shown().weighted ? ` × ${p.weight}` : ''}`}
              >
                <Meter fraction={p.window.usedPercent / 100} tone={usedTone(p.window.usedPercent)} />
              </div>
            )}
          </For>
        </div>
        <Show when={shown().leftOut.length}>
          <div class="under">Not in the total, no known plan weight: {shown().leftOut.join(', ')}</div>
        </Show>
        <Show when={shown().resetsAt}>
          {(at) => (
            <div class="under">
              Next reset <b>{until(at(), now())}</b>, {date(at())}
            </div>
          )}
        </Show>
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
      </div>
      <Show when={others().length}>
        <div class="others">
          <For each={others()}>
            {(w) => (
              <div class="other">
                <span>{w.title}</span>
                <b>{w.used}%</b>
                <span>of {w.of}%</span>
                <button class="linkish" aria-label={`Show ${w.title}`} onClick={() => pick(w.title)}>
                  Show
                </button>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
