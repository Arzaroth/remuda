import { For, Show } from 'solid-js';
import { act, store } from './state';
import { AddAccount } from './components/AddAccount';
import { Group } from './components/Group';
import { Health } from './components/Health';
import { Strip } from './components/Strip';
import { ThemeSwitch } from './components/ThemeSwitch';
import { Toast } from './components/Toast';

export function App() {
  return (
    <>
      <main>
        <div class="top">
          <div class="brand">
            <h1>remuda</h1>
            <span>Logins on this machine</span>
          </div>
          <div class="top-actions">
            <ThemeSwitch />
            <button
              class="btn"
              title="Refresh the inactive logins that expire within the hour"
              onClick={() => act('/api/refresh', {})}
            >
              Refresh tokens
            </button>
          </div>
        </div>
        <Show when={store.s}>{(s) => <Health state={s()} />}</Show>
        <Show when={store.s}>
          <Strip />
        </Show>
        <div id="providers">
          <Show
            when={!store.error}
            fallback={<div class="note">{store.error}. Open the link remuda printed when it started.</div>}
          >
            <Show when={store.s}>
              <For each={store.s!.providers}>{(p) => <Group p={p} />}</For>
            </Show>
          </Show>
        </div>
        <Show when={store.s}>
          <AddAccount />
        </Show>
      </main>
      <Toast />
    </>
  );
}
