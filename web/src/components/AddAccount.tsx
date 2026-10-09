import { createEffect, createSignal, For, on, Show } from 'solid-js';
import { api } from '../api';
import { createConfirm } from '../confirm';
import { load, store, toast } from '../state';
import { overwrite, taken } from '../usage';

type Login = { id: string; url: string; needsCode: boolean; asksFor: string; opened: boolean; name: string };

export function AddAccount(props: { provider?: string }) {
  const [provider, setProvider] = createSignal(props.provider ?? store.s!.providers[0]?.id ?? '');
  createEffect(on(() => props.provider, (p) => p && setProvider(p), { defer: true }));
  const [name, setName] = createSignal('');
  const [login, setLogin] = createSignal<Login | null>(null);
  const [code, setCode] = createSignal('');
  const confirm = createConfirm();
  let openLogin: string | null = null;
  let codeInput: HTMLInputElement | undefined;

  const done = async (l: Login, code?: string) => {
    try {
      const r = await api('/api/login/finish', { id: l.id, code });
      toast(r.message);
      if (openLogin === l.id) {
        setLogin(null);
        setName('');
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
    if (force && !confirm.ask(overwrite(creds, p, n), `${p}/${n}`)) return;
    confirm.disarm();
    if (openLogin) await api('/api/login/cancel', { id: openLogin }).catch(() => {});
    let begun: Login;
    try {
      begun = { ...(await api('/api/login', { provider: p, name: n, force })), name: n };
    } catch (e) {
      return toast((e as Error).message, true);
    }
    openLogin = begun.id;
    if (!begun.opened) window.open(begun.url, '_blank', 'noopener');
    setCode('');
    setLogin(begun);
    if (begun.needsCode) codeInput?.focus();
    else done(begun);
  };

  const copy = (url: string) =>
    navigator.clipboard.writeText(url).then(
      () => toast('Sign-in link copied'),
      () => toast('Could not copy the link', true),
    );

  return (
    <section class="add">
      <h2>Add an account</h2>
      <p>Sign in with the account in your browser and remuda stores it under the name you choose.</p>
      <form onSubmit={submit}>
        <select aria-label="CLI" value={provider()} onChange={(e) => setProvider(e.currentTarget.value)}>
          <For each={store.s!.providers}>{(p) => <option value={p.id}>{p.name}</option>}</For>
        </select>
        <input
          placeholder="Name, e.g. work"
          required
          pattern="[A-Za-z0-9_][A-Za-z0-9._\-]*"
          aria-label="Name"
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
        />
        <button class="btn go" data-armed={confirm.text() ? '1' : undefined} type="submit">
          {confirm.text() ?? 'Sign in'}
        </button>
      </form>
      <Show when={login()}>
        {(l) => {
          const copyLink = (
            <button type="button" class="linkish" onClick={() => copy(l().url)}>
              copy its link
            </button>
          );
          return (
            <div class="signin">
              <div>
                Sign in with the account to store as <b>{l().name}</b>.{' '}
                <Show
                  when={l().opened}
                  fallback={
                    <>
                      A private window keeps the account you are signed into out of the way: {copyLink} into one. If
                      no tab opened,{' '}
                      <a href={l().url} target="_blank" rel="noopener">
                        open the sign-in page
                      </a>
                      .
                    </>
                  }
                >
                  A private window opened for it, so the account you are signed into here stays put. If it did not
                  appear, {copyLink} into a private window.
                </Show>
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
