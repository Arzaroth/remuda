import { createMemo, For, Show } from 'solid-js';
import { createStore } from 'solid-js/store';
import { brand } from '../brand';
import { date, plural, until, usedTone } from '../format';
import { now, store } from '../state';
import { remember, remembered } from '../storage';
import type { Provider } from '../types';
import { combine, providerUsage } from '../usage';
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
  const usage = () => providerUsage(s(), props.p.id);
  const combined = createMemo(() => combine(creds(), usage()));
  const shown = () =>
    combined().find((w) => w.title === picks[props.p.id]) ||
    combined().reduce((a, b) => (b.used / b.of > a.used / a.of ? b : a));
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
        <div class="tile-window">{shown().title}</div>
        <div class="big">
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
            {(p) => (
              <div
                class="segment"
                classList={{ active: p.active }}
                title={`${p.name}${p.active ? ' (in use)' : ''}: ${p.window.usedPercent}%`}
              >
                <Meter fraction={p.window.usedPercent / 100} tone={usedTone(p.window.usedPercent)} />
              </div>
            )}
          </For>
        </div>
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
                <span class="of">of {w.of}%</span>
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
