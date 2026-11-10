import { createEffect, createSignal, For, on, onCleanup, onMount, Show } from 'solid-js';
import { api } from '../api';
import { createConfirm } from '../confirm';
import { load, store, toast } from '../state';
import { closeSheet } from '../sheet';
import { overwrite, taken } from '../windows';
import { ProviderIcon } from './ProviderIcon';

type Login = {
  id: string;
  url: string;
  needsCode: boolean;
  asksFor: string;
  localOnly: boolean;
  machine: string | null;
  opensIn: string | null;
  name: string;
};
type Window = 'private' | 'profile' | 'normal';
type Settings = {
  browsers: { id: string; name: string }[];
  browser: string | null;
  window: Window;
  paste: boolean;
  opensIn: string | null;
};
const windows: [Window, string][] = [
  ['private', 'Private window'],
  ['profile', "The login's own profile"],
  ['normal', 'Normal window'],
];

const remudas = (l: Login) => (l.machine ? `${l.machine}, the machine remuda runs on` : 'the machine remuda runs on');

export function AddAccount(props: { provider?: string; again?: string }) {
  const [provider, setProvider] = createSignal(props.provider ?? store.s!.providers[0]?.id ?? '');
  createEffect(on(() => props.provider, (p) => p && setProvider(p), { defer: true }));
  const [name, setName] = createSignal(props.again ?? '');
  const again = () =>
    props.again ? store.s!.credentials.find((c) => c.provider === provider() && c.name === props.again) : undefined;
  const [login, setLogin] = createSignal<Login | null>(null);
  const [code, setCode] = createSignal('');
  const confirm = createConfirm();
  const [settings, setSettings] = createSignal<Settings | null>(null);
  onMount(() => {
    api<Settings>('/api/login/settings').then(setSettings, () => {});
  });
  const choose = async (change: Partial<Pick<Settings, 'browser' | 'window' | 'paste'>>) => {
    const now = settings();
    if (!now) return;
    try {
      const next = await api<Settings>('/api/login/settings', {
        browser: change.browser !== undefined ? change.browser : now.browser,
        window: change.window ?? now.window,
        paste: change.paste ?? now.paste,
      });
      setSettings(next);
      const l = login();
      if (l) setLogin({ ...l, opensIn: next.opensIn });
    } catch (e) {
      toast((e as Error).message, true);
    }
  };
  let openLogin: string | null = null;
  let beginning = false;
  let codeInput: HTMLInputElement | undefined;
  let picker: HTMLDivElement | undefined;
  onCleanup(() => {
    if (openLogin) api('/api/login/cancel', { id: openLogin }).catch(() => {});
    openLogin = null;
  });
  const step = (ev: KeyboardEvent) => {
    const ids = store.s!.providers.map((p) => p.id);
    const at = ids.indexOf(provider());
    const by = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[ev.key];
    if (!by || at < 0) return;
    ev.preventDefault();
    setProvider(ids[(at + by + ids.length) % ids.length]);
    picker?.querySelector<HTMLButtonElement>('[aria-checked="true"]')?.focus();
  };

  const done = async (l: Login, code?: string) => {
    try {
      const r = await api('/api/login/finish', { id: l.id, code });
      toast(r.message);
      if (openLogin === l.id) {
        setLogin(null);
        setName(props.again ?? '');
        if (props.again) closeSheet();
      }
    } catch (e) {
      if (openLogin === l.id) toast((e as Error).message, true);
    }
    if (openLogin === l.id) openLogin = null;
    load();
  };

  const cancel = async (l: Login) => {
    openLogin = null;
    setLogin(null);
    await api('/api/login/cancel', { id: l.id }).catch(() => {});
  };

  const submit = async (ev: SubmitEvent) => {
    ev.preventDefault();
    const p = provider();
    const n = name().trim();
    const creds = store.s!.credentials;
    const force = taken(creds, p, n);
    if (force && !props.again && !confirm.ask(overwrite(creds, p, n), `${p}/${n}`)) return;
    confirm.disarm();
    if (beginning) return;
    beginning = true;
    let begun: Login;
    try {
      if (openLogin) await api('/api/login/cancel', { id: openLogin }).catch(() => {});
      begun = { ...(await api('/api/login', { provider: p, name: n, force, paste: settings()?.paste })), name: n };
    } catch (e) {
      return toast((e as Error).message, true);
    } finally {
      beginning = false;
    }
    openLogin = begun.id;
    setCode('');
    setLogin(begun);
    if (begun.needsCode) codeInput?.focus();
    else done(begun);
  };

  const open = async (l: Login) => {
    if (l.opensIn) {
      try {
        const r = await api<{ opened: boolean }>('/api/login/open', { id: l.id });
        if (r.opened) return toast(`Opened it in ${l.opensIn}`);
        toast(`Could not open ${l.opensIn}; use the link above if no tab opens`, true);
      } catch (e) {
        return toast((e as Error).message, true);
      }
    }
    window.open(l.url, '_blank', 'noopener');
  };

  const copy = (url: string) =>
    navigator.clipboard.writeText(url).then(
      () => toast('Sign-in link copied'),
      () => toast('Could not copy the link', true),
    );

  return (
    <section class="add">
      <Show
        when={props.again}
        fallback={
          <>
            <h2 id="add-title">Add a login</h2>
            <p>
              Pick the CLI, name the login, and sign in with that account in your browser. remuda stores it under the
              name.
            </p>
          </>
        }
      >
        <h2 id="add-title">Sign {props.again} in again</h2>
        <p>
          Sign in with {again()?.email || 'its account'} in your browser. remuda replaces the tokens of{' '}
          {`${provider()}/${props.again}`} and keeps its name, auto mode and keep-warm, and its label when the account is
          the same.
        </p>
      </Show>
      <form onSubmit={submit} classList={{ again: !!props.again }}>
        <Show when={!props.again}>
          <div class="pick-cli" role="radiogroup" aria-label="CLI" ref={picker} onKeyDown={step}>
            <For each={store.s!.providers}>
              {(p) => (
                <button
                  type="button"
                  role="radio"
                  aria-checked={provider() === p.id}
                  tabIndex={provider() === p.id ? 0 : -1}
                  onClick={() => setProvider(p.id)}
                >
                  <ProviderIcon provider={p.id} size={22} />
                  {p.name}
                </button>
              )}
            </For>
          </div>
          <input
            placeholder="Name, e.g. work"
            required
            pattern="[A-Za-z0-9_][A-Za-z0-9._\-]*"
            aria-label="Name"
            value={name()}
            onInput={(e) => setName(e.currentTarget.value)}
          />
        </Show>
        <button class="btn go" data-armed={confirm.text() ? '1' : undefined} type="submit">
          {confirm.text() ?? (props.again ? 'Sign in again' : 'Sign in')}
        </button>
      </form>
      <Show when={settings()?.opensIn}>
        <div class="opens-in">
          <span class="aside">Sign-ins open in</span>
          <select
            aria-label="Browser"
            value={settings()!.browser ?? ''}
            onChange={(e) => choose({ browser: e.currentTarget.value || null })}
          >
            <option value="">The default browser</option>
            <For each={settings()!.browsers}>{(b) => <option value={b.id}>{b.name}</option>}</For>
          </select>
          <select
            aria-label="Window"
            value={settings()!.window}
            onChange={(e) => choose({ window: e.currentTarget.value as Window })}
          >
            <For each={windows}>{([id, label]) => <option value={id}>{label}</option>}</For>
          </select>
        </div>
      </Show>
      <Show when={settings() && provider() === 'claude'}>
        <label class="opens-in">
          <input
            type="checkbox"
            checked={settings()!.paste}
            onChange={(e) => choose({ paste: e.currentTarget.checked })}
          />
          <span class="aside">Paste the code instead, to sign in from a browser on another machine than remuda's</span>
        </label>
      </Show>
      <Show when={login()}>
        {(l) => {
          return (
            <div class="signin">
              <div>
                Sign in with the account to store as <b>{l().name}</b>,{' '}
                {l().localOnly ? `in a browser on ${remudas(l())}, since it calls back there` : 'in any browser'}:
              </div>
              <a class="signin-link" href={l().url} target="_blank" rel="noopener">
                {l().url}
              </a>
              <div class="inline">
                <button type="button" class="btn go" onClick={() => open(l())}>
                  {l().opensIn ? `Open in ${l().opensIn}` : 'Open in a new tab'}
                </button>
                <button type="button" class="btn" onClick={() => copy(l().url)}>
                  Copy the link
                </button>
              </div>
              <Show
                when={l().needsCode}
                fallback={
                  <div class="inline">
                    <span class="aside">Waiting for the browser to finish signing in&hellip;</span>
                    <button class="btn" onClick={() => cancel(l())}>
                      Cancel
                    </button>
                  </div>
                }
              >
                <div class="inline">
                  <input
                    ref={codeInput}
                    placeholder={`Paste ${l().asksFor}`}
                    value={code()}
                    onInput={(e) => setCode(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && !e.isComposing) {
                        e.preventDefault();
                        done(l(), code());
                      }
                    }}
                  />
                  <button class="btn go" onClick={() => done(l(), code())}>
                    Finish sign-in
                  </button>
                  <button class="btn" onClick={() => cancel(l())}>
                    Cancel
                  </button>
                </div>
              </Show>
            </div>
          );
        }}
      </Show>
    </section>
  );
}
