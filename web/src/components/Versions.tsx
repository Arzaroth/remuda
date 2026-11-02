import { createMemo, createSignal, For, onMount, Show } from 'solid-js';
import { api } from '../api';
import { ago } from '../format';
import { sections, type Span } from '../notes';
import { now, toast } from '../state';

type Release = { version: string; published: string | null; url: string; notes: string };

type Tool = {
  id: string;
  name: string;
  url: string;
  current: string | null;
  held: string | null;
  latest: string | null;
  available: boolean;
  releases: Release[];
  error?: string;
};

type Versions = { tools: Tool[]; checkedAt: number };

const day = (iso: string) => new Date(iso).toLocaleDateString(undefined, { year: 'numeric', month: 'long', day: 'numeric' });

function Inline(props: { spans: Span[] }) {
  return (
    <For each={props.spans}>
      {(s) =>
        s.kind === 'strong' ? (
          <b>
            <Inline spans={s.spans} />
          </b>
        ) : s.kind === 'code' ? (
          <code>{s.text}</code>
        ) : s.kind === 'link' ? (
          <a href={s.href} target="_blank" rel="noopener noreferrer">
            {s.text}
          </a>
        ) : (
          s.text
        )
      }
    </For>
  );
}

function Card(props: { t: Tool; busy: boolean; onUpdate: () => void }) {
  const state = () =>
    !props.t.current ? 'Not installed' : props.t.available ? 'Update available' : props.t.latest ? 'Up to date' : 'Not checked';
  return (
    <section class="u-card v-card">
      <header class="v-head">
        <h3>{props.t.name}</h3>
        <span class="pill" classList={{ 'v-new': props.t.available }}>
          {state()}
        </span>
      </header>
      <div class="u-stats">
        <div class="u-stat">
          <span>Installed</span>
          <b>{props.t.current ? `v${props.t.current}` : '-'}</b>
        </div>
        <div class="u-stat">
          <span>Latest</span>
          <b>{props.t.latest ? `v${props.t.latest}` : '-'}</b>
        </div>
      </div>
      <Show when={props.t.error}>
        <p class="v-said bad">{props.t.error}</p>
      </Show>
      <Show when={props.t.current && props.t.held}>
        <p class="v-said">{props.t.held}</p>
      </Show>
      <div class="v-actions">
        <Show
          when={props.t.current}
          fallback={
            <a class="btn go" href={props.t.url.replace(/\/releases$/, '')} target="_blank" rel="noopener noreferrer">
              Install {props.t.name}
            </a>
          }
        >
          <button class="btn go" disabled={!props.t.available || !!props.t.held || props.busy} onClick={() => props.onUpdate()}>
            {props.busy ? 'Updating...' : props.t.available ? `Update to v${props.t.latest}` : 'Update'}
          </button>
        </Show>
        <a class="btn" href={props.t.url} target="_blank" rel="noopener noreferrer">
          Releases
        </a>
      </div>
    </section>
  );
}

function Notes(props: { t: Tool }) {
  return (
    <For each={props.t.releases}>
      {(r) => (
        <article class="v-release">
          <header class="v-release-head">
            <b class="spec">v{r.version}</b>
            <Show when={r.version === props.t.latest}>
              <span class="pill v-new">Latest</span>
            </Show>
            <Show when={r.version === props.t.current}>
              <span class="pill">Installed</span>
            </Show>
            <Show when={r.published}>
              <span class="v-date">Published {day(r.published!)}</span>
            </Show>
          </header>
          <Show when={sections(r.notes).length} fallback={<p class="v-said">No notes for this release.</p>}>
            <For each={sections(r.notes)}>
              {(s) => (
                <div class="v-section">
                  <span>
                    <Show when={s.heading}>
                      <span class="pill">{s.heading}</span>
                    </Show>
                  </span>
                  <ul>
                    <For each={s.items}>
                      {(item) => (
                        <li>
                          <Inline spans={item} />
                        </li>
                      )}
                    </For>
                  </ul>
                </div>
              )}
            </For>
          </Show>
        </article>
      )}
    </For>
  );
}

async function backOnline() {
  const until = Date.now() + 60_000;
  await new Promise((done) => setTimeout(done, 2000));
  while (Date.now() < until) {
    try {
      await api('/api/versions');
      location.reload();
      return;
    } catch {
      await new Promise((done) => setTimeout(done, 1500));
    }
  }
  toast('The page did not come back; open it again with remuda open', true);
}

export function Versions() {
  const [v, setV] = createSignal<Versions | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [checking, setChecking] = createSignal(false);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [restarting, setRestarting] = createSignal<string | null>(null);
  const [picked, setPicked] = createSignal<string | null>(null);

  const fetchVersions = async (force: boolean) => {
    setChecking(true);
    try {
      setV(await (force ? api<Versions>('/api/versions', { force: true }) : api<Versions>('/api/versions')));
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setChecking(false);
    }
  };
  onMount(() => fetchVersions(false));

  const update = async (t: Tool) => {
    setBusy(t.id);
    try {
      const r = await api<{ message: string; restarting: boolean }>('/api/versions/update', { tool: t.id });
      if (r.message) toast(r.message);
      if (r.restarting) {
        setRestarting(r.message);
        await backOnline();
        return;
      }
    } catch (e) {
      toast((e as Error).message, true);
    } finally {
      setBusy(null);
    }
    await fetchVersions(false);
  };

  const shown = createMemo(() => {
    const tools = v()?.tools ?? [];
    return tools.find((t) => t.id === picked()) ?? tools.find((t) => t.available) ?? tools[0];
  });

  return (
    <div class="page">
      <header class="page-head">
        <div>
          <h1>Versions</h1>
          <p class="page-sub">
            The remuda and TokenGauge on this machine, and what GitHub last released
            <Show when={v()}>, checked {ago(v()!.checkedAt, now())}</Show>
          </p>
        </div>
        <button class="btn v-check" disabled={checking() || !!busy()} onClick={() => fetchVersions(true)}>
          {checking() ? 'Checking...' : 'Check for updates'}
        </button>
      </header>
      <Show when={restarting()}>
        <div class="note">{restarting()}. This page reloads once it is back.</div>
      </Show>
      <Show when={error()}>
        <div class="note">{error()}</div>
      </Show>
      <Show when={v()} fallback={<Show when={!error()}>{<div class="note">Asking GitHub...</div>}</Show>}>
        <div class="u-two">
          <For each={v()!.tools}>{(t) => <Card t={t} busy={busy() === t.id} onUpdate={() => update(t)} />}</For>
        </div>
        <Show when={shown()}>
          {(t) => (
            <section class="u-card v-notes">
              <header class="v-head">
                <h3>{t().available ? `What's new since v${t().current}` : 'Release notes'}</h3>
                <div class="chips" role="group" aria-label="Release notes of">
                  <For each={v()!.tools}>
                    {(o) => (
                      <button type="button" class="chip" aria-pressed={o.id === t().id} onClick={() => setPicked(o.id)}>
                        {o.name}
                      </button>
                    )}
                  </For>
                </div>
              </header>
              <Show when={t().releases.length} fallback={<p class="v-said">{t().error ? 'GitHub could not be reached.' : 'No release yet.'}</p>}>
                <Notes t={t()} />
              </Show>
              <footer class="v-foot">
                <a href={t().url} target="_blank" rel="noopener noreferrer">
                  Every {t().name} release
                </a>
              </footer>
            </section>
          )}
        </Show>
      </Show>
    </div>
  );
}
