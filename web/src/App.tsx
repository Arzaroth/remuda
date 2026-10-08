import { createSignal, For, Show } from 'solid-js';
import { act, store } from './state';
import { remember, remembered } from './storage';
import { AddAccount } from './components/AddAccount';
import { Group } from './components/Group';
import { Health } from './components/Health';
import { Strip } from './components/Strip';
import { Tabs } from './components/Tabs';
import { ThemeSwitch } from './components/ThemeSwitch';
import { Toast } from './components/Toast';

export function App() {
  const [tab, setTab] = createSignal(remembered<string>('remuda-tab') || 'all');
  const pickTab = (id: string) => {
    setTab(id);
    remember('remuda-tab', id);
  };
  const shown = () => {
    const providers = store.s!.providers;
    const only = providers.filter((p) => p.id === tab());
    return only.length ? only : providers;
  };
  return (
    <>
      <main>
        <div class="top">
          <div class="brand">
            <img src="/favicon.svg" alt="" width="30" height="30" />
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
        <Show when={store.s}>
          <Tabs value={shown().length === 1 ? shown()[0].id : 'all'} onChange={pickTab} />
        </Show>
        <div id="providers">
          <Show
            when={!store.error}
            fallback={<div class="note">{store.error}. Open the link remuda printed when it started.</div>}
          >
            <Show when={store.s}>
              <For each={shown()}>{(p) => <Group p={p} titled={shown().length > 1} />}</For>
            </Show>
          </Show>
        </div>
        <Show when={store.s}>
          <AddAccount provider={shown().length === 1 ? shown()[0].id : undefined} />
        </Show>
      </main>
      <Toast />
    </>
  );
}
