import { createEffect, createMemo, createSignal, For, on, Show } from 'solid-js';
import { api } from '../api';
import { byLabel, days, subject } from '../log';
import { now, store } from '../state';
import type { LogEvent } from '../types';
import { inUse } from '../usage';
import { ProviderIcon } from './ProviderIcon';

const day = (ms: number) => new Date(ms).toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' });
const time = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });

function Row(props: { e: LogEvent }) {
  return (
    <li class="log-row" classList={{ bad: !props.e.ok }}>
      <time datetime={new Date(props.e.at).toISOString()} title={new Date(props.e.at).toLocaleString()}>
        {time(props.e.at)}
      </time>
      <span class="pill">{props.e.kind}</span>
      <span class="log-subject">
        <Show when={props.e.provider}>
          <ProviderIcon provider={props.e.provider!} size={16} />
        </Show>
        <span class="spec">{subject(props.e)}</span>
      </span>
      <span class="log-message">
        <Show when={!props.e.ok}>
          <b>Failed: </b>
        </Show>
        {props.e.message}
        <Show when={props.e.by}>
          <span class="log-by"> {byLabel[props.e.by!]}</span>
        </Show>
      </span>
    </li>
  );
}

export function Log() {
  const [events, setEvents] = createSignal<LogEvent[] | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [provider, setProvider] = createSignal<string | null>(null);
  const [failures, setFailures] = createSignal(false);
  let fetches = 0;
  createEffect(
    on(now, () => {
      const seq = ++fetches;
      api<{ events: LogEvent[] }>('/api/log').then(
        (r) => {
          if (seq !== fetches) return;
          setEvents(r.events);
          setError(null);
        },
        (e) => seq === fetches && setError((e as Error).message),
      );
    }),
  );
  const clis = createMemo(() => {
    const named = new Set((events() ?? []).map((e) => e.provider));
    return store.s!.providers.filter((p) => named.has(p.id) || inUse(store.s!).some((u) => u.id === p.id));
  });
  const shown = createMemo(() => days(events() ?? [], { provider: provider(), failures: failures() }));
  return (
    <div class="page">
      <header class="page-head">
        <div>
          <h1>Log</h1>
          <p class="page-sub">What remuda did, newest first: switches and who made them, refreshes, auto mode, keep-warm and the proxy</p>
        </div>
      </header>
      <div class="chips log-filters" role="group" aria-label="Show">
        <button type="button" class="chip" aria-pressed={provider() === null} onClick={() => setProvider(null)}>
          Every CLI
        </button>
        <For each={clis()}>
          {(p) => (
            <button type="button" class="chip" aria-pressed={provider() === p.id} onClick={() => setProvider(p.id)}>
              {p.name}
            </button>
          )}
        </For>
        <button type="button" class="chip" aria-pressed={failures()} onClick={() => setFailures(!failures())}>
          Failures only
        </button>
      </div>
      <Show when={error()}>
        <div class="note">{error()}</div>
      </Show>
      <Show when={events()}>
        <Show
          when={shown().length}
          fallback={
            <div class="note">
              {events()!.length ? 'Nothing logged matches.' : 'Nothing logged yet. Switches, refreshes and what auto mode does land here.'}
            </div>
          }
        >
          <div class="log">
            <For each={shown()}>
              {(d) => (
                <section>
                  <h2>{day(d.at)}</h2>
                  <ul>
                    <For each={d.events}>{(e) => <Row e={e} />}</For>
                  </ul>
                </section>
              )}
            </For>
          </div>
        </Show>
      </Show>
      <p class="page-sub log-more">
        <code>remuda log</code> goes further back.
      </p>
    </div>
  );
}
