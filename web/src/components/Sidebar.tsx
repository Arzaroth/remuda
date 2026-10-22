import { For, Show } from 'solid-js';
import { act, store } from '../state';
import { href, route, type Route } from '../route';
import type { Provider } from '../types';
import { autoOn, inUse } from '../usage';
import { usedTone } from '../format';
import { healthTone } from './Health';
import { Meter } from './Meter';
import { ProviderIcon } from './ProviderIcon';
import { lead } from './Strip';
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

export function Sidebar(props: { onAdd: () => void }) {
  const s = () => store.s!;
  const tone = () => healthTone(s());
  return (
    <nav class="side" aria-label="remuda">
      <a class="side-brand" href={href({ page: 'overview' })}>
        <img src="/favicon.svg" alt="" width="30" height="30" />
        <span>remuda</span>
      </a>
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
        <a class="nav-item" href={href({ page: 'status' })} aria-current={here({ page: 'status' }) ? 'page' : undefined}>
          <svg viewBox="0 0 20 20" aria-hidden="true">
            <path d="M2 10h4l2-5 4 10 2-5h4" />
          </svg>
          Status
          <span class={`nav-dot ${tone() || 'good'}`} />
          <span class="sr">{{ '': 'all good', warn: 'needs a look', bad: 'has problems' }[tone()]}</span>
        </a>
      </div>
      <div class="side-foot">
        <button class="btn go wide" onClick={() => props.onAdd()}>
          Add a login
        </button>
        <button
          class="btn wide"
          title="Refresh the logins not in use whose tokens expire within the hour"
          onClick={() => act('/api/refresh', {})}
        >
          Refresh tokens
        </button>
        <ThemeSwitch />
        <SkinSwitch />
      </div>
    </nav>
  );
}
