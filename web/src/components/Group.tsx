import { createSignal, For, Match, onMount, Show, Switch } from 'solid-js';
import { brand } from '../brand';
import { createConfirm } from '../confirm';
import { date, DAY, HOUR, span, until, usedTone } from '../format';
import { act, now, store } from '../state';
import type { Credential, Live, Provider, Shown, Window } from '../types';
import { dropping, dropsLive, overwrite, providerUsage, taken, usageOf } from '../usage';
import { Icon } from './Icon';
import { Meter } from './Meter';

export function Group(props: { p: Provider; titled: boolean }) {
  const s = () => store.s!;
  const creds = () => s().credentials.filter((c) => c.provider === props.p.id);
  const live = () => s().live.find((l) => l.provider === props.p.id);
  const usage = () => providerUsage(s(), props.p.id);
  return (
    <section class="group">
      <Show when={props.titled}>
        <div class="group-head">
          <span class="brand-dot" style={{ background: brand(props.p.id) }} />
          <h2>{props.p.name}</h2>
        </div>
      </Show>
      <Show when={live()}>{(l) => <LiveNote p={props.p} live={l()} />}</Show>
      <div class="cards">
        <For
          each={creds()}
          fallback={<div class="empty">No stored {props.p.name} logins. Import the one in use or add an account below.</div>}
        >
          {(c) => <Row c={c} usage={usage()} live={live()} />}
        </For>
      </div>
    </section>
  );
}

const skipped: Record<string, string> = {
  expired: 'Its access token has expired, so TokenGauge waits for remuda to refresh it',
  unverified: 'TokenGauge skips it until remuda identifies it again',
};

function WindowCell(props: { w: Window }) {
  const resets = () => (props.w.resetsAt ? Date.parse(props.w.resetsAt) : null);
  return (
    <div class="win">
      <div class="win-head">
        <span class="what" title={props.w.title}>
          {props.w.title}
        </span>
        <b>{props.w.usedPercent}%</b>
        <Show when={resets()} fallback={<span class="when">no reset</span>}>
          {(at) => (
            <span class="when" title={`Resets ${date(at())}`}>
              {until(at(), now())}
            </span>
          )}
        </Show>
      </div>
      <Meter fraction={props.w.usedPercent / 100} tone={usedTone(props.w.usedPercent)} />
    </div>
  );
}

function Windows(props: { c: Credential; usage: Shown | null }) {
  const u = () => usageOf(props.c, props.usage);
  return (
    <Show when={store.s!.health.tokengauge}>
      <Switch>
        <Match when={u()?.passive}>
          <div class="aside">TokenGauge reports only the login in use</div>
        </Match>
        <Match when={u()?.missing}>
          <div class="aside">No usage from TokenGauge for this login yet</div>
        </Match>
        <Match when={!u()}>
          <div class="aside">No usage from TokenGauge for this CLI yet</div>
        </Match>
        <Match when={u()!.behind}>
          <div class="aside">TokenGauge has not updated since the switch to this login</div>
        </Match>
        <Match when={u()!.error}>
          <div class="aside late">TokenGauge: {u()!.error}</div>
        </Match>
        <Match when={u()!.credentialState}>
          {(state) => (
            <div class="aside soon">{skipped[state()] || `TokenGauge did not read it (${state()})`}</div>
          )}
        </Match>
        <Match when={!u()!.windows.length}>
          <div class="aside">No rate limits reported</div>
        </Match>
        <Match when={true}>
          <div class="windows">
            <For each={u()!.windows}>{(w) => <WindowCell w={w} />}</For>
          </div>
          <Show when={u()!.stale}>
            <div class="aside soon">
              TokenGauge could not update this{u()!.staleReason ? ` (${u()!.staleReason})` : ''}; figures may be old
            </div>
          </Show>
        </Match>
      </Switch>
    </Show>
  );
}

function Lifetime(props: { title: string; at: number | null; horizon: number; warnBelow: number }) {
  return (
    <Show when={props.at != null}>
      {(_) => {
        const left = () => props.at! - now();
        const tone = () => (left() < 0 ? 'bad' : left() < props.warnBelow ? 'warn' : '');
        const cls = () => (left() < 0 ? 'late' : left() < props.warnBelow ? 'soon' : '');
        return (
          <div class="win" title={date(props.at!)}>
            <div class="win-head">
              <span class="what">{props.title}</span>
              <b class={cls()}>{left() < 0 ? 'expired' : span(left())}</b>
            </div>
            <Meter fraction={left() / props.horizon} tone={tone()} />
          </div>
        );
      }}
    </Show>
  );
}

type Editing = { kind: 'label' | 'rename'; original: string; value: string };

function Editor(props: {
  class: string;
  rename: boolean;
  value: string;
  label: string;
  onInput: (v: string) => void;
  onSave: () => void;
  onLeave: () => void;
  ref: (el: HTMLInputElement) => void;
}) {
  let input!: HTMLInputElement;
  onMount(() => {
    input.focus();
    input.select();
  });
  return (
    <span class={props.class}>
      <input
        ref={(el) => {
          input = el;
          props.ref(el);
        }}
        class={props.rename ? 'edit spec' : 'edit'}
        value={props.value}
        placeholder={props.rename ? 'Name' : 'Label'}
        aria-label={props.label}
        onInput={(e) => props.onInput(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') props.onSave();
          if (e.key === 'Escape') props.onLeave();
        }}
      />
      <button class="ok" aria-label="Save" title="Save (Enter)" onClick={() => props.onSave()}>
        <Icon name="save" />
      </button>
    </span>
  );
}

function Row(props: { c: Credential; usage: Shown | null; live: Live | undefined }) {
  const c = () => props.c;
  const unverified = () => c().verified === false;
  const spec = () => `${c().provider}/${c().name}`;
  const [editing, setEditing] = createSignal<Editing | null>(null);
  let input: HTMLInputElement | undefined;
  const confirms = { use: createConfirm(), remove: createConfirm(), label: createConfirm(), rename: createConfirm() };

  const open = (kind: Editing['kind']) => {
    const original = kind === 'rename' ? c().name : c().label || '';
    setEditing({ kind, original, value: original });
  };
  const close = () => {
    const e = editing();
    if (e) confirms[e.kind].disarm();
    setEditing(null);
  };
  const leave = () => {
    const e = editing()!;
    if (e.value.trim() === e.original.trim() || confirms[e.kind].ask('Discard?')) {
      close();
      return true;
    }
    input?.focus();
    return false;
  };
  const toggle = (kind: Editing['kind']) => {
    const e = editing();
    if (e?.kind === kind) return leave();
    if (e && !leave()) return;
    open(kind);
  };
  const save = () => {
    const e = editing()!;
    const value = e.kind === 'rename' ? e.value.trim() : e.value;
    close();
    if (value.trim() === e.original.trim()) return;
    return e.kind === 'rename'
      ? act('/api/rename', { name: spec(), to: value })
      : act('/api/label', { name: spec(), text: value });
  };
  const use = () => {
    const live = props.live;
    const discard = dropsLive(live);
    if (discard && !confirms.use.ask(dropping(live!), spec())) return;
    return act('/api/use', { name: spec(), discard });
  };
  const remove = () => {
    if (!confirms.remove.ask('Remove for good?')) return;
    return act('/api/remove', { name: spec() });
  };
  const editor = (kind: Editing['kind']) => (
    <Editor
      class={kind === 'rename' ? 'editing' : 'editing edit-label'}
      rename={kind === 'rename'}
      value={editing()!.value}
      label={kind === 'rename' ? `New name for ${editing()!.original}` : `Label for ${spec()}`}
      onInput={(value) => setEditing({ ...editing()!, value })}
      onSave={save}
      onLeave={leave}
      ref={(el) => (input = el)}
    />
  );
  const iconButton = (kind: 'label' | 'rename', title: string, disabled: boolean) => (
    <button
      class="btn icon"
      classList={{ armed: !!confirms[kind].text() }}
      data-armed={confirms[kind].text() ? '1' : undefined}
      aria-label={`${title} ${c().name}`}
      aria-pressed={editing()?.kind === kind ? 'true' : undefined}
      title={title}
      disabled={disabled}
      onClick={() => toggle(kind)}
    >
      {confirms[kind].text() ?? <Icon name={kind} />}
    </button>
  );

  return (
    <article class="card" classList={{ active: c().active }}>
      <header class="who">
        <div class="name-line">
          <Show when={editing()?.kind === 'rename'} fallback={<span class="spec">{c().name}</span>}>
            {editor('rename')}
          </Show>
          <Show when={editing()?.kind === 'label'} fallback={<span class="label">{c().label || ''}</span>}>
            {editor('label')}
          </Show>
        </div>
        <div class="mail">{c().email}</div>
        <div class="pills">
          <Show when={c().plan}>
            <span class="pill">{c().plan}</span>
          </Show>
          <Show when={c().active}>
            <span class="pill on">In use</span>
          </Show>
          <Show when={unverified()}>
            <span
              class="pill odd"
              title="Its sidecar was written for other tokens; remuda identifies it again on its next run"
            >
              Unverified
            </span>
          </Show>
        </div>
      </header>
      <div class="usage">
        <Windows c={c()} usage={props.usage} />
      </div>
      <div class="tokens">
        <Lifetime title="Access token" at={c().expiresAt} horizon={8 * HOUR} warnBelow={HOUR} />
        <Lifetime title="Sign-in lasts" at={c().refreshTokenExpiresAt} horizon={30 * DAY} warnBelow={3 * DAY} />
      </div>
      <footer class="actions">
        <button
          class="btn go"
          classList={{ armed: !!confirms.use.text() }}
          data-armed={confirms.use.text() ? '1' : undefined}
          disabled={c().active || unverified()}
          onClick={use}
        >
          {confirms.use.text() ?? 'Use'}
        </button>
        <button
          class="btn icon"
          aria-label={`Refresh ${c().name}`}
          disabled={c().active || unverified()}
          title={c().active ? 'The CLI refreshes the login in use itself' : unverified() ? undefined : 'Refresh its tokens now'}
          onClick={() => act('/api/refresh', { name: spec(), force: true })}
        >
          <Icon name="refresh" />
        </button>
        {iconButton('label', 'Label', unverified())}
        {iconButton('rename', 'Rename', false)}
        <button
          class="btn icon danger"
          classList={{ armed: !!confirms.remove.text() }}
          data-armed={confirms.remove.text() ? '1' : undefined}
          aria-label={`Remove ${c().name}`}
          disabled={c().active}
          title={c().active ? 'Switch to another login first' : 'Remove'}
          onClick={remove}
        >
          {confirms.remove.text() ?? <Icon name="remove" />}
        </button>
      </footer>
    </article>
  );
}

function LiveNote(props: { p: Provider; live: Live }) {
  const [name, setName] = createSignal('');
  const confirm = createConfirm();
  let input!: HTMLInputElement;
  const doImport = () => {
    const provider = props.p.id;
    const n = name().trim();
    if (!n) return input.focus();
    const creds = store.s!.credentials;
    const force = taken(creds, provider, n);
    if (force && !confirm.ask(overwrite(creds, provider, n), `${provider}/${n}`)) return;
    return act('/api/import', { provider, name: n, force });
  };
  const l = () => props.live;
  return (
    <Switch>
      <Match when={l().state === 'signed_out'}>
        <div class="note">{props.p.name} is signed out. Pick a login above to use it.</div>
      </Match>
      <Match when={l().state === 'unreadable'}>
        <div class="note">
          Cannot read the {props.p.name} login: {l().error}
        </div>
      </Match>
      <Match when={l().state === 'foreign'}>
        <div class="note">
          {props.p.name} is signed in with {l().what}, which remuda cannot store. Using a stored login replaces it,
          after a second click, since it is dropped.
        </div>
      </Match>
      <Match when={l().state === 'stored' && !l().confirmed}>
        <div class="note">
          {props.p.name} looks signed into {l().name}, but that could not be confirmed. Using another login asks
          first, since it would be dropped unsaved.
        </div>
      </Match>
      <Match when={l().state === 'unstored'}>
        <div class="note">
          {props.p.name} is signed into {l().email || 'an account'}, which remuda has not stored.
          <span class="inline">
            <input
              ref={input}
              placeholder="Name to store it as"
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.isComposing) {
                  e.preventDefault();
                  doImport();
                }
              }}
            />
            <button
              class="btn go"
              classList={{ armed: !!confirm.text() }}
              data-armed={confirm.text() ? '1' : undefined}
              onClick={doImport}
            >
              {confirm.text() ?? 'Import'}
            </button>
          </span>
        </div>
      </Match>
    </Switch>
  );
}
