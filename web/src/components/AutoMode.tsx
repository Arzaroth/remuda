import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from 'solid-js';
import { createStore, produce, reconcile, unwrap } from 'solid-js/store';
import { api } from '../api';
import { brand } from '../brand';
import {
  auto,
  canonical,
  type Config,
  DAYS,
  dropFromRule,
  headline,
  hours,
  inRule,
  type Loaded,
  minutes,
  OTHERWISE,
  percent,
  type Problem,
  problemsAt,
  renameSchedule,
  scheduleName,
  scheduleUses,
  span,
} from '../config';
import { ago } from '../format';
import { now, store, toast } from '../state';
import type { AutoStatus, Provider } from '../types';

const [saved, setSaved] = createSignal<Loaded | null>(null);
const [draft, setDraft] = createStore<{ c: Config }>({ c: {} });
const [conflict, setConflict] = createSignal(false);
const [loadError, setLoadError] = createSignal<string | null>(null);

const text = (c: Config) => canonical(c);
const dirty = () => !!saved() && text(draft.c) !== text(saved()!.config);
const edit = (fn: (c: Config) => void) => setDraft('c', produce(fn));

function take(l: Loaded) {
  setSaved(l);
  setDraft('c', reconcile(structuredClone(l.config)));
  setConflict(false);
  setLoadError(null);
}

export async function loadConfig() {
  try {
    const l = await api<Loaded>('/api/config');
    if (!saved() || !dirty()) take(l);
    else if (l.version !== saved()!.version) setConflict(true);
  } catch (e) {
    setLoadError((e as Error).message);
  }
}

const loginsOf = (provider: string) =>
  store.s!.credentials.filter((c) => c.provider === provider).map((c) => c.name);

function Chip(props: { name: string; on?: boolean; onClick: () => void; title?: string; remove?: boolean }) {
  return (
    <button
      type="button"
      class="chip"
      aria-pressed={props.remove ? undefined : !!props.on}
      title={props.title}
      onClick={props.onClick}
    >
      <span class="spec">{props.name}</span>
      <Show when={props.remove}>
        <span aria-hidden="true" class="x">
          &times;
        </span>
      </Show>
    </button>
  );
}

function Problems(props: { list: Problem[] }) {
  return (
    <Show when={props.list.length}>
      <ul class="problems">
        <For each={props.list}>
          {(p) => (
            <li>
              <code>{p.at}</code> {p.message}
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}

function RuleCard(props: { provider: string; i: number; count: number; problems: Problem[] }) {
  const rule = () => draft.c.auto![props.provider].rule![props.i];
  const at = () => `auto.${props.provider}.rule[${props.i + 1}]`;
  const schedules = () => Object.keys(draft.c.schedules ?? {});
  const free = () => loginsOf(props.provider).filter((n) => !inRule(rule()).includes(n));
  const change = (fn: (r: NonNullable<ReturnType<typeof rule>>) => void) =>
    edit((c) => fn(auto(c, props.provider).rule![props.i]));
  const move = (by: number) =>
    edit((c) => {
      const rules = auto(c, props.provider).rule!;
      const [r] = rules.splice(props.i, 1);
      rules.splice(props.i + by, 0, r);
    });
  return (
    <div class="rule">
      <div class="rule-head">
        <b>Rule {props.i + 1}</b>
        <label class="inline">
          <span class="aside">when</span>
          <select
            aria-label={`Rule ${props.i + 1} applies`}
            value={rule().when}
            onChange={(e) => change((r) => (r.when = e.currentTarget.value))}
          >
            <For each={schedules()}>{(s) => <option value={s}>{s}</option>}</For>
            <option value={OTHERWISE}>any other time</option>
            <Show when={rule().when !== OTHERWISE && !schedules().includes(rule().when)}>
              <option value={rule().when}>{rule().when} (missing)</option>
            </Show>
          </select>
        </label>
        <span class="rule-tools">
          <button class="btn icon" title="Move up" disabled={props.i === 0} onClick={() => move(-1)}>
            &uarr;
          </button>
          <button class="btn icon" title="Move down" disabled={props.i === props.count - 1} onClick={() => move(1)}>
            &darr;
          </button>
          <button
            class="btn icon danger"
            title="Remove this rule"
            onClick={() => edit((c) => auto(c, props.provider).rule!.splice(props.i, 1))}
          >
            &times;
          </button>
        </span>
      </div>
      <For each={rule().order}>
        {(tier, j) => (
          <div class="tier">
            <span class="aside tier-name">{j() === 0 ? 'First' : `Then`}</span>
            <For each={tier}>
              {(name) => (
                <Chip
                  name={name}
                  remove
                  title={`Take ${name} out of this rule`}
                  onClick={() => change((r) => dropFromRule(r, name))}
                />
              )}
            </For>
            <Show when={free().length}>
              <select
                class="add-login"
                aria-label="Add a login to this tier"
                value=""
                onChange={(e) => {
                  const name = e.currentTarget.value;
                  e.currentTarget.value = '';
                  if (name) change((r) => r.order[j()].push(name));
                }}
              >
                <option value="">+ login</option>
                <For each={free()}>{(n) => <option value={n}>{n}</option>}</For>
              </select>
            </Show>
            <Show when={!tier.length}>
              <button
                class="linkish aside"
                onClick={() => change((r) => r.order.splice(j(), 1))}
                title="Remove this empty tier"
              >
                remove
              </button>
            </Show>
          </div>
        )}
      </For>
      <div class="tier">
        <button class="linkish aside" onClick={() => change((r) => r.order.push([]))}>
          + a tier, used when every login above is maxed
        </button>
      </div>
      <Show when={inRule(rule()).length && schedules().length}>
        <div class="reserve">
          <span class="aside">Kept for a schedule:</span>
          <For each={inRule(rule())}>
            {(name) => (
              <label class="kept">
                <span class="spec">{name}</span>
                <select
                  aria-label={`Keep ${name} for`}
                  value={rule().reserve?.[name] ?? ''}
                  onChange={(e) => {
                    const s = e.currentTarget.value;
                    change((r) => {
                      if (s) (r.reserve ??= {})[name] = s;
                      else if (r.reserve) delete r.reserve[name];
                    });
                  }}
                >
                  <option value="">never kept</option>
                  <For each={schedules()}>{(s) => <option value={s}>{s}</option>}</For>
                </select>
              </label>
            )}
          </For>
        </div>
      </Show>
      <Problems list={problemsAt(props.problems, at())} />
    </div>
  );
}

function Now(props: { s: AutoStatus; provider: string }) {
  const active = () => store.s!.live.find((l) => l.provider === props.provider)?.name ?? null;
  const head = () => headline(props.s, active());
  const d = () => props.s.decision;
  return (
    <div class="auto-now">
      <div class="verdict">
        <span class={`dot ${head().tone}`} />
        <b>{head().text}</b>
        <Show when={d().rule}>
          <span class="aside">{d().rule}</span>
        </Show>
      </div>
      <div class="aside why">{props.s.error ?? d().action.why}</div>
      <Show when={d().logins.length}>
        <table class="seen">
          <tbody>
            <For each={d().logins}>
              {(l) => (
                <tr classList={{ off: !l.usable, active: l.name === active() }}>
                  <td class="aside">{l.tier === null ? '-' : l.tier + 1}</td>
                  <td class="spec">{l.name}</td>
                  <td class="num">{l.score === null ? '' : `${l.score.toFixed(1)}%/h`}</td>
                  <td class="aside">{[l.name === active() ? 'in use' : '', l.note].filter(Boolean).join(', ')}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
      <Show when={props.s.switched}>
        {(sw) => (
          <div class="aside">
            Switched to <span class="spec">{sw().to}</span> {ago(sw().at, now())}: {sw().why}
          </div>
        )}
      </Show>
    </div>
  );
}

function ProviderCard(props: { p: Provider; problems: Problem[] }) {
  const status = () => {
    const s = store.s!.auto?.[props.p.id];
    return s && typeof s === 'object' && 'decision' in s ? s : null;
  };
  const a = () => draft.c.auto?.[props.p.id] ?? {};
  const set = (fn: (a: ReturnType<typeof auto>) => void) => edit((c) => fn(auto(c, props.p.id)));
  const at = () => `auto.${props.p.id}`;
  const own = () =>
    props.problems.filter(
      (p) => (p.at === at() || p.at.startsWith(`${at()}.`)) && !p.at.startsWith(`${at()}.rule[`),
    );
  const addRule = () =>
    set((x) =>
      (x.rule ??= []).push({ when: Object.keys(draft.c.schedules ?? {})[0] ?? OTHERWISE, order: [[]] }),
    );
  return (
    <div class="auto-card" style={{ '--brand': brand(props.p.id) }}>
      <div class="auto-card-head">
        <span class="brand-dot" style={{ background: brand(props.p.id) }} />
        <h3>{props.p.name}</h3>
        <button
          class="switch"
          role="switch"
          aria-checked={!!a().enabled}
          onClick={() => set((x) => (x.enabled = !x.enabled))}
        >
          <i />
          {a().enabled ? 'On' : 'Off'}
        </button>
      </div>
      <Show when={status()}>{(s) => <Now s={s()} provider={props.p.id} />}</Show>
      <Show when={props.p.id === 'codex'}>
        <p class="aside">A running Codex session cannot take a new login, so auto mode waits for none to be open.</p>
      </Show>
      <div class="fields">
        <label>
          <span class="aside">Stay at least</span>
          <span class="unit">
            <input
              type="number"
              min="0"
              step="5"
              value={minutes(a().dwell)}
              onChange={(e) =>
                set((x) => {
                  x.dwell = span(Math.max(0, Math.round(+e.currentTarget.value || 0)));
                  if (x.dwell === '30m') delete x.dwell;
                })
              }
            />
            min
          </span>
        </label>
        <label>
          <span class="aside">Leave only for a pick</span>
          <span class="unit">
            <input
              type="number"
              min="0"
              step="5"
              value={Math.round((a().margin ?? 0.25) * 100)}
              onChange={(e) =>
                set((x) => {
                  x.margin = Math.max(0, +e.currentTarget.value || 0) / 100;
                  if (x.margin === 0.25) delete x.margin;
                })
              }
            />
            % better
          </span>
        </label>
        <label>
          <span class="aside">A window is full at</span>
          <span class="unit">
            <input
              type="number"
              min="1"
              max="100"
              value={a().ceiling ?? 100}
              onChange={(e) => {
                const c = percent(e.currentTarget.value) ?? 100;
                e.currentTarget.value = String(c);
                set((x) => {
                  x.ceiling = c;
                  if (c === 100) delete x.ceiling;
                });
              }}
            />
            %
          </span>
        </label>
      </div>
      <div class="pickable">
        <span class="aside">Logins it may pick when no rule is in force</span>
        <div class="chips">
          <For each={loginsOf(props.p.id)} fallback={<span class="aside">No stored logins</span>}>
            {(name) => (
              <Chip
                name={name}
                on={(a().logins ?? []).includes(name)}
                onClick={() =>
                  set((x) => {
                    const l = (x.logins ??= []);
                    const i = l.indexOf(name);
                    if (i >= 0) l.splice(i, 1);
                    else l.push(name);
                  })
                }
              />
            )}
          </For>
        </div>
      </div>
      <Show when={loginsOf(props.p.id).length}>
        <div class="pickable">
          <span class="aside">A login's own ceiling for its shortest window (Claude's 5-hour one)</span>
          <div class="ceilings">
            <For each={loginsOf(props.p.id)}>
              {(name) => (
                <label class="kept">
                  <span class="spec">{name}</span>
                  <span class="unit">
                    <input
                      type="number"
                      min="1"
                      max="100"
                      aria-label={`${name} is full at`}
                      placeholder={String(a().ceiling ?? 100)}
                      value={a().ceilings?.[name] ?? ''}
                      onChange={(e) => {
                        const c = percent(e.currentTarget.value);
                        e.currentTarget.value = c === null ? '' : String(c);
                        set((x) => {
                          if (c !== null) (x.ceilings ??= {})[name] = c;
                          else if (x.ceilings) {
                            delete x.ceilings[name];
                            if (!Object.keys(x.ceilings).length) delete x.ceilings;
                          }
                        });
                      }}
                    />
                    %
                  </span>
                </label>
              )}
            </For>
          </div>
        </div>
        <label class="kept">
          <span class="aside">When every login it may pick is full</span>
          <select
            aria-label="When every login is full"
            value={a().fallback ?? ''}
            onChange={(e) => {
              const f = e.currentTarget.value;
              set((x) => {
                if (f) x.fallback = f;
                else delete x.fallback;
              });
            }}
          >
            <option value="">stay where it is</option>
            <For each={loginsOf(props.p.id)}>{(n) => <option value={n}>go to {n}</option>}</For>
          </select>
        </label>
      </Show>
      <For each={a().rule ?? []}>
        {(_, i) => <RuleCard provider={props.p.id} i={i()} count={a().rule!.length} problems={props.problems} />}
      </For>
      <button class="btn" onClick={addRule}>
        Add a rule
      </button>
      <Problems list={own()} />
    </div>
  );
}

function ScheduleRow(props: { name: string }) {
  const s = () => draft.c.schedules![props.name];
  const h = () => hours(s().hours);
  const uses = () => scheduleUses(draft.c, props.name);
  const set = (fn: (s: NonNullable<Config['schedules']>[string]) => void) =>
    edit((c) => fn(c.schedules![props.name]));
  const rename = (to: string) => {
    to = to.trim();
    if (to === props.name) return;
    const why = scheduleName(draft.c, to);
    if (why) return toast(`Not renamed: ${why}`, true);
    edit((c) => renameSchedule(c, props.name, to));
  };
  return (
    <div class="schedule">
      <div class="schedule-head">
        <input
          class="spec name"
          aria-label="Schedule name"
          value={props.name}
          onChange={(e) => rename(e.currentTarget.value)}
        />
        <button
          class="btn icon danger"
          title={uses().length ? `Used by ${uses().join(', ')}` : 'Remove this schedule'}
          disabled={!!uses().length}
          onClick={() => edit((c) => delete c.schedules![props.name])}
        >
          &times;
        </button>
      </div>
      <div class="days" role="group" aria-label="Days">
        <For each={DAYS}>
          {([d, label]) => (
            <button
              type="button"
              aria-pressed={(s().days ?? []).includes(d)}
              onClick={() =>
                set((x) => {
                  const days = new Set(x.days ?? []);
                  if (days.has(d)) days.delete(d);
                  else days.add(d);
                  x.days = DAYS.map(([k]) => k).filter((k) => days.has(k));
                  if (!x.days.length) delete x.days;
                })
              }
            >
              {label}
            </button>
          )}
        </For>
      </div>
      <div class="inline hours">
        <label class="check">
          <input
            type="checkbox"
            checked={!s().hours}
            onChange={(e) =>
              set((x) => {
                if (e.currentTarget.checked) delete x.hours;
                else x.hours = '09:00-18:00';
              })
            }
          />
          all day
        </label>
        <Show when={h()}>
          {(hh) => (
            <>
              <input
                type="time"
                aria-label="From"
                value={hh().start}
                onChange={(e) => set((x) => (x.hours = `${e.currentTarget.value}-${hh().end}`))}
              />
              <span class="aside">to</span>
              <input
                type="time"
                aria-label="To"
                value={hh().end}
                onChange={(e) => set((x) => (x.hours = `${hh().start}-${e.currentTarget.value}`))}
              />
            </>
          )}
        </Show>
      </div>
    </div>
  );
}

function Schedules(props: { problems: Problem[] }) {
  const [name, setName] = createSignal('');
  const add = (ev: SubmitEvent) => {
    ev.preventDefault();
    const n = name().trim();
    const why = scheduleName(draft.c, n);
    if (why) return toast(`Not added: ${why}`, true);
    edit((c) => ((c.schedules ??= {})[n] = { days: ['mon', 'tue', 'wed', 'thu', 'fri'], hours: '09:00-18:00' }));
    setName('');
  };
  return (
    <div class="auto-card">
      <div class="auto-card-head">
        <h3>Schedules</h3>
      </div>
      <p class="aside">Local time. A range that ends before it starts runs past midnight.</p>
      <For each={Object.keys(draft.c.schedules ?? {})}>{(n) => <ScheduleRow name={n} />}</For>
      <form class="inline" onSubmit={add}>
        <input placeholder="Name, e.g. workday" aria-label="New schedule" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        <button class="btn" type="submit">
          Add a schedule
        </button>
      </form>
      <Problems list={[...problemsAt(props.problems, 'schedules'), ...problemsAt(props.problems, 'config')]} />
    </div>
  );
}

export function AutoMode(props: { providers: Provider[] }) {
  const [problems, setProblems] = createSignal<Problem[]>([]);
  const [saving, setSaving] = createSignal(false);
  if (!saved()) loadConfig();

  let timer: ReturnType<typeof setTimeout> | undefined;
  let checks = 0;
  createEffect(
    on(
      () => [text(draft.c), saved()],
      () => {
        clearTimeout(timer);
        const seq = ++checks;
        if (!saved()) return;
        if (!dirty()) return setProblems(saved()!.problems);
        const config = structuredClone(unwrap(draft.c));
        timer = setTimeout(async () => {
          try {
            const r = await api<{ problems: Problem[] }>('/api/config/check', { config });
            if (seq === checks) setProblems(r.problems);
          } catch {}
        }, 250);
      },
    ),
  );
  onCleanup(() => clearTimeout(timer));

  const save = async () => {
    setSaving(true);
    try {
      take(await api<Loaded>('/api/config', { config: unwrap(draft.c), version: saved()!.version }));
      toast('Saved config.toml');
    } catch (e) {
      const m = (e as Error).message;
      if (m.includes('changed since it was read')) setConflict(true);
      toast(m, true);
    }
    setSaving(false);
  };
  const reloadFile = async () => {
    setSaved(null);
    await loadConfig();
  };
  const elsewhere = createMemo(() => {
    const shown = new Set(props.providers.map((p) => p.id));
    return problems().filter((p) => {
      const m = p.at.match(/^auto\.([^.[]+)/);
      return m && !shown.has(m[1]);
    });
  });

  return (
    <section class="auto">
      <div class="auto-head">
        <h2>Auto mode</h2>
        <Show when={saved()}>
          <span class="aside">
            <code>{saved()!.path}</code>
          </span>
        </Show>
      </div>
      <p class="aside">
        remuda keeps each CLI on the most suitable login you allow, and moves it off one that is maxed. Within a tier,
        it spends first the headroom a reset is about to wipe.
      </p>
      <Show when={typeof store.s!.auto?.error === 'string'}>
        <div class="note">Auto mode is stopped: {store.s!.auto!.error as string}</div>
      </Show>
      <Show when={loadError()}>
        <div class="note">
          Could not read the config: {loadError()}{' '}
          <button class="linkish" onClick={reloadFile}>
            Try again
          </button>
        </div>
      </Show>
      <Show when={conflict()}>
        <div class="note">
          The config file changed outside this page.{' '}
          <button class="linkish" onClick={reloadFile}>
            Load it
          </button>
          , dropping the edits here.
        </div>
      </Show>
      <Show when={saved()}>
        <div class="auto-grid">
          <For each={props.providers}>{(p) => <ProviderCard p={p} problems={problems()} />}</For>
          <Schedules problems={problems()} />
        </div>
        <Problems list={elsewhere()} />
      </Show>
      <Show when={dirty()}>
        <div class="savebar">
          <span>
            Unsaved changes
            <Show when={problems().length}>
              , <span class="late">{problems().length === 1 ? '1 problem' : `${problems().length} problems`}</span>
            </Show>
          </span>
          <button class="btn" onClick={reloadFile}>
            Revert
          </button>
          <button class="btn go" disabled={saving() || !!problems().length} onClick={save}>
            Save
          </button>
        </div>
      </Show>
    </section>
  );
}
