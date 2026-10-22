import { createEffect, createSignal, Match, Show, Switch } from 'solid-js';
import { act, store } from './state';
import { go, href, route } from './route';
import type { Provider } from './types';
import { autoOn, inUse } from './usage';
import { AddAccount } from './components/AddAccount';
import { AutoMode, dirty } from './components/AutoMode';
import { Logins } from './components/Group';
import { Health } from './components/Health';
import { Proxy } from './components/Proxy';
import { ProviderIcon } from './components/ProviderIcon';
import { Sidebar } from './components/Sidebar';
import { Strip, Tile } from './components/Strip';
import { SkinSwitch } from './components/SkinSwitch';
import { ThemeSwitch } from './components/ThemeSwitch';
import { Toast } from './components/Toast';

function AutoBadge(props: { p: Provider }) {
  const on = () => autoOn(store.s!, props.p.id);
  return (
    <a class="auto-badge" classList={{ on: on() }} href={href({ page: 'cli', id: props.p.id, tab: 'automation' })}>
      {on() ? 'Auto mode on' : 'Auto mode off'}
    </a>
  );
}

function CliPage(props: { p: Provider; tab: 'logins' | 'automation' }) {
  const active = () => store.s!.credentials.find((c) => c.provider === props.p.id && c.active);
  const count = () => store.s!.credentials.filter((c) => c.provider === props.p.id).length;
  return (
    <div class="page">
      <header class="page-head">
        <ProviderIcon provider={props.p.id} size={40} />
        <div>
          <h1>{props.p.name}</h1>
          <p class="page-sub">
            <Show when={active()} fallback={<>No stored login in use, </>}>
              <b class="spec">{active()!.name}</b> in use,{' '}
            </Show>
            {count()} {count() === 1 ? 'login' : 'logins'} stored
          </p>
        </div>
        <Show when={props.tab === 'logins'}>
          <AutoBadge p={props.p} />
        </Show>
      </header>
      <nav class="page-tabs" aria-label={props.p.name}>
        <a
          aria-current={props.tab === 'logins' ? 'page' : undefined}
          href={href({ page: 'cli', id: props.p.id, tab: 'logins' })}
        >
          Logins
        </a>
        <a
          aria-current={props.tab === 'automation' ? 'page' : undefined}
          href={href({ page: 'cli', id: props.p.id, tab: 'automation' })}
        >
          Automation
          <Show when={dirty()}>
            <span class="unsaved">unsaved</span>
          </Show>
        </a>
      </nav>
      <Switch>
        <Match when={props.tab === 'logins'}>
          <Tile p={props.p} bare />
          <Logins p={props.p} />
        </Match>
        <Match when={props.tab === 'automation'}>
          <AutoMode providers={[props.p]} />
        </Match>
      </Switch>
    </div>
  );
}

export function App() {
  let dialog!: HTMLDialogElement;
  const [adding, setAdding] = createSignal<string | undefined>();
  const providers = () => (store.s ? inUse(store.s) : []);
  const cli = () => {
    const r = route();
    if (r.page !== 'cli' || !store.s) return null;
    const p = store.s.providers.find((x) => x.id === r.id);
    return p ? { p, tab: r.tab } : null;
  };
  const openAdd = () => {
    const r = route();
    setAdding(r.page === 'cli' ? r.id : providers()[0]?.id);
    dialog.showModal();
    dialog.querySelector<HTMLElement>('[role="radio"][aria-checked="true"]')?.focus();
  };
  createEffect(() => {
    const r = route();
    if (r.page === 'cli' && store.s && !store.s.providers.some((p) => p.id === r.id)) go({ page: 'overview' });
  });
  return (
    <>
      <div class="shell">
        <Show when={store.s}>
          <Sidebar onAdd={openAdd} />
        </Show>
        <main>
          <Show
            when={!store.error}
            fallback={<div class="note">{store.error}. Open the link remuda printed when it started.</div>}
          >
            <Show when={store.s}>
              <Switch>
                <Match when={cli()}>{(c) => <CliPage p={c().p} tab={c().tab} />}</Match>
                <Match when={route().page === 'proxy' && store.s!.proxy}>
                  <div class="page">
                    <Proxy />
                  </div>
                </Match>
                <Match when={route().page === 'status'}>
                  <div class="page">
                    <header class="page-head">
                      <div>
                        <h1>Status</h1>
                        <p class="page-sub">The refresh timer, TokenGauge, and where the logins are stored</p>
                      </div>
                    </header>
                    <Health state={store.s!} />
                    <div class="phone-only">
                      <button
                        class="btn"
                        title="Refresh the logins not in use whose tokens expire within the hour"
                        onClick={() => act('/api/refresh', {})}
                      >
                        Refresh tokens
                      </button>
                      <ThemeSwitch />
                      <SkinSwitch />
                    </div>
                  </div>
                </Match>
                <Match when={true}>
                  <div class="page">
                    <header class="page-head">
                      <div>
                        <h1>Overview</h1>
                        <p class="page-sub">What each CLI is spending, summed over every stored login</p>
                      </div>
                    </header>
                    <Strip />
                  </div>
                </Match>
              </Switch>
            </Show>
          </Show>
        </main>
      </div>
      <dialog ref={dialog} class="sheet" aria-labelledby="add-title" onClose={() => setAdding(undefined)}>
        <button class="sheet-close" aria-label="Close" onClick={() => dialog.close()}>
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
        <Show when={store.s && adding() !== undefined}>
          <AddAccount provider={adding()} />
        </Show>
      </dialog>
      <Toast />
    </>
  );
}
