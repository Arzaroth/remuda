import { createEffect, createSignal, For, onCleanup, onMount, Show } from 'solid-js';
import { act, store } from '../state';
import { href, route, type Route } from '../route';
import type { Provider } from '../types';
import { autoOn, inUse } from '../windows';
import { usedTone } from '../format';
import { healthTone } from './Health';
import { Meter } from './Meter';
import { ProviderIcon } from './ProviderIcon';
import { lead } from './Strip';
import { api } from '../api';
import { noteVersions, updates, type Checked } from '../updates';
import { SkinSwitch } from './SkinSwitch';
import { ThemeSwitch } from './ThemeSwitch';

const here = (r: Route) => {
  const now = route();
  return now.page === r.page && (r.page !== 'cli' || (now.page === 'cli' && now.id === r.id));
};

function CliLink(props: { p: Provider }) {
  const s = () => store.s!;
  const active = () => s().credentials.find((c) => c.provider === props.p.id && c.active);
  const live = () => s().live.find((l) => l.provider === props.p.id);
  const w = () => lead(props.p.id);
  const auto = () => autoOn(s(), props.p.id);
  const r: Route = { page: 'cli', id: props.p.id, tab: 'logins' };
  return (
    <a class="nav-cli" href={href(r)} aria-current={here(r) ? 'page' : undefined}>
      <ProviderIcon provider={props.p.id} size={22} />
      <span class="nav-cli-text">
        <span class="nav-name">
          {props.p.name}
          <Show when={auto()}>
            <span class="auto-tag" title="Auto mode is on">
              auto
            </span>
          </Show>
        </span>
        <span class="nav-sub">
          {active()?.name ?? (live()?.state === 'signed_out' ? 'signed out' : 'no stored login in use')}
          <Show when={w()}>{(x) => <b>{Math.round((x().used / x().of) * 100)}%</b>}</Show>
        </span>
        <Show when={w()}>{(x) => <Meter fraction={x().used / x().of} tone={usedTone((x().used / x().of) * 100)} />}</Show>
      </span>
    </a>
  );
}

const PLACES: Record<Exclude<Route['page'], 'cli'>, string> = {
  overview: 'Overview',
  usage: 'Usage',
  quota: 'Quota',
  proxy: 'API proxy',
  log: 'Log',
  status: 'Status',
  versions: 'Versions',
};

export function Sidebar(props: { onAdd: () => void }) {
  const s = () => store.s!;
  const tone = () => healthTone(s());
  // The server keeps GitHub's answer a quarter of an hour, so asking often
  // costs GitHub nothing more; a failed check leaves the dot as it was.
  const checkUpdates = () =>
    api<Checked>('/api/versions')
      .then(noteVersions)
      .catch(() => {});
  onMount(() => {
    checkUpdates();
    const every = setInterval(checkUpdates, 30 * 60_000);
    onCleanup(() => clearInterval(every));
  });
  const [open, setOpen] = createSignal(false);
  const place = () => {
    const r = route();
    return r.page === 'cli' ? (s().providers.find((p) => p.id === r.id)?.name ?? '') : PLACES[r.page];
  };
  createEffect(() => {
    route();
    setOpen(false);
  });
  const escape = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
  window.addEventListener('keydown', escape);
  onCleanup(() => window.removeEventListener('keydown', escape));
  return (
    <nav class="side" classList={{ open: open() }} aria-label="remuda">
      <a class="side-brand" href={href({ page: 'overview' })}>
        <img src="/favicon.svg" alt="" width="30" height="30" />
        <span>remuda</span>
      </a>
      <span class="side-here">{place()}</span>
      <button
        class="menu-btn"
        aria-label="Menu"
        aria-expanded={open()}
        aria-controls="side-menu"
        onClick={() => setOpen(!open())}
      >
        <svg viewBox="0 0 20 20" aria-hidden="true">
          <path d="M3 3h5v5H3zM12 3h5v5h-5zM3 12h5v5H3zM12 12h5v5h-5z" />
        </svg>
      </button>
      <Show when={open()}>
        <div class="menu-scrim" onClick={() => setOpen(false)} />
      </Show>
      <div class="side-menu" id="side-menu">
        <div class="nav-group">
          <a class="nav-item" href={href({ page: 'overview' })} aria-current={here({ page: 'overview' }) ? 'page' : undefined}>
            <svg viewBox="0 0 20 20" aria-hidden="true">
              <path d="M3 3h6v6H3zM11 3h6v6h-6zM3 11h6v6H3zM11 11h6v6h-6z" />
            </svg>
            Overview
          </a>
        </div>
        <div class="nav-group nav-clis">
          <For each={inUse(s())}>{(p) => <CliLink p={p} />}</For>
        </div>
        <div class="nav-group">
          <a class="nav-item" href={href({ page: 'usage', tab: 'spend' })} aria-current={here({ page: 'usage', tab: 'spend' }) ? 'page' : undefined}>
            <svg viewBox="0 0 20 20" aria-hidden="true">
              <path d="M3 17V9M8 17V4M13 17v-6M18 17V7" />
            </svg>
            Usage
          </a>
          <a class="nav-item" href={href({ page: 'quota', id: null })} aria-current={here({ page: 'quota', id: null }) ? 'page' : undefined}>
            <svg viewBox="0 0 20 20" aria-hidden="true">
              <path d="M3 4v13h14M5 13l4-4 3 3 5-6" />
            </svg>
            Quota
          </a>
          <Show when={s().proxy}>
            <a class="nav-item" href={href({ page: 'proxy' })} aria-current={here({ page: 'proxy' }) ? 'page' : undefined}>
              <svg viewBox="0 0 20 20" aria-hidden="true">
                <path d="M3 7h12l-3-3M17 13H5l3 3" />
              </svg>
              API proxy
              <span class="nav-dot" classList={{ good: !!s().proxy?.enabled }} />
              <span class="sr">{s().proxy?.enabled ? 'on' : 'off'}</span>
            </a>
          </Show>
          <a class="nav-item" href={href({ page: 'log' })} aria-current={here({ page: 'log' }) ? 'page' : undefined}>
            <svg viewBox="0 0 20 20" aria-hidden="true">
              <path d="M4 5h12M4 10h12M4 15h8" />
            </svg>
            Log
          </a>
          <a class="nav-item" href={href({ page: 'status' })} aria-current={here({ page: 'status' }) ? 'page' : undefined}>
            <svg viewBox="0 0 20 20" aria-hidden="true">
              <path d="M2 10h4l2-5 4 10 2-5h4" />
            </svg>
            Status
            <span class={`nav-dot ${tone() || 'good'}`} />
            <span class="sr">{{ '': 'all good', warn: 'needs a look', bad: 'has problems' }[tone()]}</span>
          </a>
          <a class="nav-item" href={href({ page: 'versions' })} aria-current={here({ page: 'versions' }) ? 'page' : undefined}>
            <svg viewBox="0 0 20 20" aria-hidden="true">
              <path d="M10 15V5M6 9l4-4 4 4M4 17h12" />
            </svg>
            Versions
            <Show when={updates().length}>
              <span class="nav-dot new" />
              <span class="sr">{updates().join(' and ')} can be updated</span>
            </Show>
          </a>
        </div>
        <div class="side-foot">
          <button
            class="btn go wide"
            onClick={() => {
              setOpen(false);
              props.onAdd();
            }}
          >
            Add a login
          </button>
          <button
            class="btn wide"
            title="Refresh the logins not in use whose tokens expire within the hour"
            onClick={() => act('/api/refresh', {})}
          >
            Refresh tokens
          </button>
          <div class="switches">
            <ThemeSwitch />
            <SkinSwitch />
          </div>
        </div>
      </div>
    </nav>
  );
}
