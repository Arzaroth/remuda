import { createSignal, For, Match, Show, Switch } from 'solid-js';
import { ago, MIN, plural } from '../format';
import { now } from '../state';
import type { State } from '../types';

export function Health(props: { state: State }) {
  const [open, setOpen] = createSignal(false);
  const h = () => props.state.health;
  const run = () => h().lastRefresh;
  const count = () => run()?.problems?.length || 0;
  const tone = () => {
    const r = run();
    return !r || now() - r.at > 75 * MIN ? 'warn' : r.problems.length ? 'bad' : 'good';
  };
  return (
    <div class="health">
      <div class="health-line">
        <Switch>
          <Match when={h().timer === 'enabled'}>
            <div>
              <span class={`dot ${tone()}`} />
              <span>
                {run() ? `Refresh timer ran ${ago(run()!.at, now())}, refreshed ${run()!.refreshed.length}` : 'Refresh timer on, it has not run yet'}
                <Show when={count()}>
                  ,{' '}
                  <button class="linkish" aria-expanded={open()} aria-controls="problems" onClick={() => setOpen(!open())}>
                    {plural(count(), 'problem')}
                  </button>
                </Show>
              </span>
            </div>
          </Match>
          <Match when={h().timer === 'masked'}>
            <div>
              <span class="dot warn" />
              <span>
                Refresh timer masked: <code>systemctl --user unmask remuda-refresh.timer</code>
              </span>
            </div>
          </Match>
          <Match when={h().timer === 'disabled'}>
            <div>
              <span class="dot warn" />
              <span>
                Refresh timer installed but off: <code>systemctl --user enable --now remuda-refresh.timer</code>
              </span>
            </div>
          </Match>
          <Match when={true}>
            <div>
              <span class="dot warn" />
              <span>No refresh timer; spare logins stay fresh only when you press Refresh tokens</span>
            </div>
          </Match>
        </Switch>
        <Show
          when={h().tokengauge}
          fallback={
            <div>
              <span class="dot" />
              <span>
                Install{' '}
                <a href="https://github.com/Arzaroth/TokenGauge" target="_blank" rel="noopener noreferrer">
                  TokenGauge
                </a>{' '}
                to see usage here
              </span>
            </div>
          }
        >
          <div>
            <span class="dot good" />
            <span>
              Usage from TokenGauge
              {props.state.usage?.updatedAt ? `, updated ${ago(props.state.usage.updatedAt, now())}` : ''}
            </span>
          </div>
        </Show>
        <div>
          <span>
            Store <code>{props.state.store}</code>
          </span>
        </div>
      </div>
      <Show when={count()}>
        <ul class="problems" id="problems" hidden={!open()}>
          <For each={run()!.problems}>{(l) => <li>{l}</li>}</For>
        </ul>
      </Show>
    </div>
  );
}
