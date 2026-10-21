import { createSignal, For, Show } from 'solid-js';
import { brand } from '../brand';
import { createConfirm } from '../confirm';
import { act, store, toast } from '../state';
import { remember, remembered } from '../storage';
import { baseUrl, masked, PROTOCOLS, status, type Protocol } from '../proxy';
import { Icon } from './Icon';

const copy = (text: string, what: string) =>
  navigator.clipboard.writeText(text).then(
    () => toast(`${what} copied`),
    () => toast(`Could not copy the ${what.toLowerCase()}`, true),
  );

export function Proxy() {
  const p = () => store.s!.proxy;
  const [protocol, setProtocol] = createSignal<Protocol>(remembered<Protocol>('remuda-proxy-protocol') || 'openai');
  const [shown, setShown] = createSignal(false);
  const [managing, setManaging] = createSignal(false);
  const [picked, setPicked] = createSignal<string | null>(null);
  const [name, setName] = createSignal('');
  const [port, setPort] = createSignal('');
  const [armed, setArmed] = createSignal<string | null>(null);
  const confirm = createConfirm();

  const pick = (id: Protocol) => {
    setProtocol(id);
    remember('remuda-proxy-protocol', id);
  };
  const key = () => p().keys.find((k) => k.id === picked()) ?? p().keys[0];
  const format = () => PROTOCOLS.find((x) => x.id === protocol())!.format;

  const addKey = async (ev: SubmitEvent) => {
    ev.preventDefault();
    const n = name().trim();
    if (!n) return;
    await act('/api/proxy/keys', { name: n });
    setName('');
  };
  const removeKey = (id: string, n: string) => {
    if (!confirm.ask(`Remove the key ${n}? Clients using it stop working.`, id)) return setArmed(id);
    act('/api/proxy/keys/remove', { id });
  };
  const savePort = (ev: SubmitEvent) => {
    ev.preventDefault();
    const n = Number(port());
    if (!Number.isInteger(n) || n < 1 || n > 65535) return toast('A port is a number from 1 to 65535', true);
    act('/api/proxy/settings', { port: n }).then(() => setPort(''));
  };

  return (
    <section class="proxy">
      <div class="proxy-head">
        <div>
          <h2>API proxy</h2>
          <p>
            One local endpoint for any OpenAI or Anthropic client. The model name picks the CLI whose active login answers,
            and the path tells the format.
          </p>
        </div>
        <label class="switch">
          <input
            type="checkbox"
            checked={p().enabled}
            onChange={(e) => act('/api/proxy/settings', { enabled: e.currentTarget.checked })}
          />
          <span>{p().enabled ? 'On' : 'Off'}</span>
        </label>
      </div>
      <div class={`proxy-status ${status(p()).tone}`}>{status(p()).text}</div>

      <div class="field-label">Choose the API protocol used by your client</div>
      <div class="segmented" role="group" aria-label="Protocol">
        <For each={PROTOCOLS}>
          {(x) => (
            <button aria-pressed={protocol() === x.id} onClick={() => pick(x.id)}>
              <span class="dot" style={{ background: brand(x.id === 'claude' ? 'claude' : 'codex') }} />
              {x.name}
            </button>
          )}
        </For>
      </div>

      <div class="field-label">
        <span>API base URL</span>
        <span class="hint">{format()}</span>
      </div>
      <div class="field">
        <code>{baseUrl(p(), protocol())}</code>
        <button class="btn icon" title="Copy the base URL" onClick={() => copy(baseUrl(p(), protocol()), 'Base URL')}>
          <Icon name="copy" />
        </button>
      </div>

      <div class="field-label">
        <span>Access key</span>
        <button class="link" onClick={() => setManaging(!managing())}>
          {managing() ? 'Done' : 'Manage keys'}
        </button>
      </div>
      <Show
        when={key()}
        fallback={<div class="field empty">No key yet: clients are refused until one is made under Manage keys.</div>}
      >
        {(k) => (
          <div class="field">
            <Show when={p().keys.length > 1}>
              <select aria-label="Key" value={k().id} onChange={(e) => setPicked(e.currentTarget.value)}>
                <For each={p().keys}>{(x) => <option value={x.id}>{x.name}</option>}</For>
              </select>
            </Show>
            <code>{shown() ? k().key : masked(k().key)}</code>
            <button
              class="btn icon"
              aria-pressed={shown()}
              title={shown() ? 'Hide the key' : 'Show the key'}
              onClick={() => setShown(!shown())}
            >
              <Icon name="show" />
            </button>
            <button class="btn icon" title="Copy the key" onClick={() => copy(k().key, 'Key')}>
              <Icon name="copy" />
            </button>
          </div>
        )}
      </Show>

      <Show when={managing()}>
        <div class="keys">
          <For each={p().keys}>
            {(k) => (
              <div class="key-row">
                <span class="key-name">{k.name}</span>
                <span class="muted">made {new Date(k.created).toLocaleDateString()}</span>
                <button class="btn danger" onClick={() => removeKey(k.id, k.name)}>
                  {confirm.text() !== null && armed() === k.id ? 'Click again' : 'Remove'}
                </button>
              </div>
            )}
          </For>
          <Show when={confirm.text()}>
            <div class="muted">{confirm.text()}</div>
          </Show>
          <form class="inline" onSubmit={addKey}>
            <input placeholder="Key name, e.g. laptop" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
            <button class="btn go" disabled={!name().trim()}>
              Make a key
            </button>
          </form>
          <form class="inline" onSubmit={savePort}>
            <input
              inputmode="numeric"
              placeholder={`Port, now ${p().port}`}
              value={port()}
              onInput={(e) => setPort(e.currentTarget.value)}
            />
            <button class="btn" disabled={!port().trim()}>
              Change port
            </button>
          </form>
        </div>
      </Show>

      <Show when={p().routes.length}>
        <div class="field-label">Models it routes</div>
        <ul class="routes">
          <For each={p().routes}>
            {(r) => (
              <li>
                <span class="dot" style={{ background: brand(r.provider) }} />
                <code>{r.models.join(', ')}</code>
                <span class="muted">{r.formats.join(' · ')}</span>
              </li>
            )}
          </For>
        </ul>
        <p class="muted small">
          Any model with the same prefix routes the same way; <code>provider/model</code> names a provider outright.
        </p>
      </Show>
    </section>
  );
}
