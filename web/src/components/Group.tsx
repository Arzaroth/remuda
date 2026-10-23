import { createSignal, For, Match, onMount, Show, Switch } from 'solid-js';
import { createConfirm } from '../confirm';
import { ago, date, DAY, HOUR, plural, span, until, usedTone } from '../format';
import { openSheet } from '../sheet';
import { act, now, store } from '../state';
import type { Credential, Live, Provider, Window } from '../types';
import { carriedSince, dropping, dropsLive, overwrite, planFor, shownFor, taken } from '../usage';
import { warmLine } from '../warm';
import { remember, remembered } from '../storage';
import { Icon } from './Icon';
import { Meter } from './Meter';

type Order = 'store' | 'headroom' | 'name';
const orders: Order[] = ['store', 'headroom', 'name'];
const kept = remembered<Order>('remuda-order');
const [order, setOrder] = createSignal<Order>(kept && orders.includes(kept) ? kept : 'store');

const figures = (c: Credential) => {
  const u = shownFor(store.s!, c);
  return u && !u.error && !u.missing && !u.passive ? u.windows : [];
};

const headroom = (c: Credential) => {
  const w = figures(c);
  return w.length ? Math.min(...w.map((x) => 100 - x.usedPercent)) : -Infinity;
};

export function Logins(props: { p: Provider }) {
  const s = () => store.s!;
  const creds = () => {
    const all = s().credentials.filter((c) => c.provider === props.p.id);
    if (order() === 'name') return [...all].sort((a, b) => a.name.localeCompare(b.name));
    if (order() === 'headroom') return [...all].sort((a, b) => headroom(b) - headroom(a) || 0);
    return all;
  };
  const live = () => s().live.find((l) => l.provider === props.p.id);
  const pick = (o: Order) => {
    setOrder(o);
    remember('remuda-order', o);
  };
  return (
    <section class="logins">
      <Show when={live()}>{(l) => <LiveNote p={props.p} live={l()} />}</Show>
      <Show when={creds().length > 1}>
        <div class="list-tools">
          <span class="aside">Order</span>
          <div class="seg" role="group" aria-label="Order logins">
            <button aria-pressed={order() === 'store'} onClick={() => pick('store')}>
              As stored
            </button>
            <button aria-pressed={order() === 'headroom'} onClick={() => pick('headroom')}>
              Most headroom
            </button>
            <button aria-pressed={order() === 'name'} onClick={() => pick('name')}>
              Name
            </button>
          </div>
        </div>
      </Show>
      <div class="list">
        <For
          each={creds()}
          fallback={
            <div class="empty">
              No stored {props.p.name} logins yet. Import the one in use above, or add one with Add a login.
            </div>
          }
        >
          {(c) => <Row c={c} live={live()} />}
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

function Windows(props: { c: Credential }) {
  const u = () => shownFor(store.s!, props.c);
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
          <Show when={carriedSince(store.s!, u()!)}>
            {(at) => (
              <div class="aside" title={date(at())}>
                Figures from {ago(at(), now())}
              </div>
            )}
          </Show>
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

function Compact(props: { c: Credential }) {
  return (
    <div class="compact">
      <For each={figures(props.c)} fallback={<span class="aside">no figures</span>}>
        {(w) => (
          <div class="cw" title={`${w.title}: ${w.usedPercent}%${w.resetsAt ? `, resets ${until(Date.parse(w.resetsAt), now())}` : ''}`}>
            <span class="cw-title">{w.title}</span>
            <b classList={{ late: w.usedPercent >= 100 }}>{w.usedPercent}%</b>
            <Meter fraction={w.usedPercent / 100} tone={usedTone(w.usedPercent)} />
          </div>
        )}
      </For>
    </div>
  );
}

function Banked(props: { c: Credential }) {
  const resets = () => {
    const r = shownFor(store.s!, props.c)?.bankedResets;
    return r && !(r.expiresAt && Date.parse(r.expiresAt) <= now()) ? r : null;
  };
  const confirm = createConfirm();
  const [spentWith, setSpentWith] = createSignal<string | null>(null);
  const held = () => spentWith() !== null && spentWith() === JSON.stringify(resets());
  const spend = () => {
    if (held() || !confirm.ask(`Spend ${props.c.name}'s?`)) return;
    setSpentWith(JSON.stringify(resets()));
    return act('/api/reset', { name: `${props.c.provider}/${props.c.name}` });
  };
  return (
    <Show when={resets()}>
      {(r) => (
        <div class="banked">
          <span class="aside">
            {plural(r().available, 'banked reset')}
            <Show when={r().expiresAt && !Number.isNaN(Date.parse(r().expiresAt!)) && r().expiresAt}>
              {(at) => <span title={date(Date.parse(at()))}>, expires {until(Date.parse(at()), now())}</span>}
            </Show>
          </span>
          <button
            class="btn"
            data-armed={confirm.text() ? '1' : undefined}
            title={held() ? 'Waiting for TokenGauge to report what is left' : 'Clears the session and weekly limits now'}
            disabled={held()}
            onClick={spend}
          >
            {confirm.text() ?? 'Reset limits'}
          </button>
        </div>
      )}
    </Show>
  );
}

function Lifetime(props: { title: string; at: number | null; horizon: number; warnBelow: number }) {
  const left = () => props.at! - now();
  const tone = () => (left() < 0 ? 'bad' : left() < props.warnBelow ? 'warn' : '');
  return (
    <Show when={props.at != null}>
      <div class="win" title={date(props.at!)}>
        <div class="win-head">
          <span class="what">{props.title}</span>
          <b classList={{ late: tone() === 'bad', soon: tone() === 'warn' }}>{left() < 0 ? 'expired' : span(left())}</b>
        </div>
        <Meter fraction={left() / props.horizon} tone={tone()} />
      </div>
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
          if (e.isComposing) return;
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

function Row(props: { c: Credential; live: Live | undefined }) {
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
      class="btn"
      data-armed={confirms[kind].text() ? '1' : undefined}
      aria-label={`${title} ${c().name}`}
      aria-pressed={editing()?.kind === kind ? 'true' : undefined}
      title={title}
      disabled={disabled}
      onClick={() => toggle(kind)}
    >
      {confirms[kind].text() ?? (
        <>
          <Icon name={kind} /> {title}
        </>
      )}
    </button>
  );

  const [expanded, setExpanded] = createSignal(false);
  const warm = () => store.s!.warm?.[spec()];
  const signedOut = () => c().refreshTokenExpiresAt != null && c().refreshTokenExpiresAt! <= now();
  const tokenLeft = () => (c().expiresAt == null ? null : c().expiresAt! - now());

  return (
    <article class="login" classList={{ active: c().active, open: expanded() }}>
      <div class="login-main">
        <span class="state" classList={{ on: c().active }} aria-hidden="true" />
        <div class="who">
          <div class="name-line">
            <Show when={editing()?.kind === 'rename'} fallback={<span class="spec">{c().name}</span>}>
              {editor('rename')}
            </Show>
            <Show when={editing()?.kind === 'label'} fallback={<span class="label">{c().label || ''}</span>}>
              {editor('label')}
            </Show>
          </div>
          <div class="mail">{c().email}</div>
        </div>
        <div class="pills">
          <Show when={planFor(store.s!, c())}>{(plan) => <span class="pill">{plan()}</span>}</Show>
          <Show when={unverified()}>
            <span class="pill odd" title="Its sidecar was written for other tokens; remuda identifies it again on its next run">
              Unverified
            </span>
          </Show>
          <Show when={c().running}>
            <span class="pill run-pill" title="A `remuda run` session uses it: it is not refreshed, switched to or removed until it ends">
              In a session
            </span>
          </Show>
          <Show when={warm()}>
            {(w) => (
              <span class={`pill warm-pill ${w().state === 'failed' ? 'bad' : ''}`} title={warmLine(w(), now()).text}>
                Kept warm
              </span>
            )}
          </Show>
        </div>
        <Compact c={c()} />
        <div class="token-left" title="Access token">
          <Show when={tokenLeft() != null} fallback={<span class="aside">&nbsp;</span>}>
            <span class="aside">token</span>
            <b classList={{ late: tokenLeft()! < 0, soon: tokenLeft()! >= 0 && tokenLeft()! < HOUR }}>
              {tokenLeft()! < 0 ? 'expired' : span(tokenLeft()!)}
            </b>
          </Show>
        </div>
        <div class="use">
          <Show
            when={!c().active}
            fallback={<span class="in-use">In use</span>}
          >
            <button
              class="btn go"
              data-armed={confirms.use.text() ? '1' : undefined}
              disabled={unverified() || c().running}
              onClick={use}
            >
              {confirms.use.text() ?? 'Use'}
            </button>
          </Show>
        </div>
        <button
          class="more"
          aria-expanded={expanded()}
          aria-label={`${expanded() ? 'Hide' : 'Show'} details for ${c().name}`}
          onClick={() => setExpanded(!expanded())}
        >
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
            <path d="M4 6l4 4 4-4" />
          </svg>
        </button>
      </div>
      <Show when={expanded()}>
        <div class="login-detail">
          <div class="detail-usage">
            <Windows c={c()} />
            <Banked c={c()} />
          </div>
          <div class="detail-side">
            <div class="tokens">
              <Lifetime title="Access token" at={c().expiresAt} horizon={8 * HOUR} warnBelow={HOUR} />
              <Lifetime title="Sign-in lasts" at={c().refreshTokenExpiresAt} horizon={30 * DAY} warnBelow={3 * DAY} />
            </div>
            <Show when={warm()}>
              {(w) => {
                const line = () => warmLine(w(), now());
                return <div class={`aside warm ${line().tone}`}>{line().text}</div>;
              }}
            </Show>
            <div class="actions">
              <button
                class="btn"
                disabled={c().active || unverified() || c().running}
                title={
                  c().active
                    ? 'The CLI refreshes the login in use itself'
                    : c().running
                      ? 'Its `remuda run` session refreshes it'
                      : unverified()
                        ? undefined
                        : 'Refresh its tokens now'
                }
                onClick={() => act('/api/refresh', { name: spec(), force: true })}
              >
                <Icon name="refresh" /> Refresh
              </button>
              <button
                class={signedOut() ? 'btn go' : 'btn'}
                disabled={c().active || c().running}
                title={
                  c().active
                    ? 'Sign in again through the CLI itself, then import it'
                    : c().running
                      ? 'End its `remuda run` session first'
                      : 'Sign this login in again in the browser'
                }
                onClick={() => openSheet({ provider: c().provider, again: c().name })}
              >
                <Icon name="signin" /> Sign in again
              </button>
              {iconButton('label', 'Label', unverified())}
              {iconButton('rename', 'Rename', false)}
              <button
                class="btn danger"
                data-armed={confirms.remove.text() ? '1' : undefined}
                disabled={c().active || c().running}
                title={c().active ? 'Switch to another login first' : c().running ? 'End its `remuda run` session first' : 'Remove'}
                onClick={remove}
              >
                {confirms.remove.text() ?? (
                  <>
                    <Icon name="remove" /> Remove
                  </>
                )}
              </button>
            </div>
          </div>
        </div>
      </Show>
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
        <div class="note">{props.p.name} is signed out. Pick a login below to use it.</div>
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
