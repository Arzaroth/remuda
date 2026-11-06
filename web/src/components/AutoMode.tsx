import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from 'solid-js';
import { createStore, produce, reconcile, unwrap } from 'solid-js/store';
import { api } from '../api';
import { brand } from '../brand';
import {
  advancedOnly,
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
  moveLogin,
  oneCeiling,
  ordered,
  OTHERWISE,
  percent,
  type Problem,
  problemsAt,
  renameSchedule,
  scheduleName,
  scheduleUses,
  setCeiling,
  simplify,
  span,
  toggleLogin,
  toggleWarm,
} from '../config';
import { ago } from '../format';
import { now, store, toast } from '../state';
import { createConfirm } from '../confirm';
import { remember, remembered } from '../storage';
import { liveOf, loginsOf, ownWindows } from '../windows';
import type { AutoStatus, Provider } from '../types';

const [saved, setSaved] = createSignal<Loaded | null>(null);
const [draft, setDraft] = createStore<{ c: Config }>({ c: {} });
const [conflict, setConflict] = createSignal(false);
const [loadError, setLoadError] = createSignal<string | null>(null);

const text = (c: Config) => canonical(c);
export const dirty = () => !!saved() && text(draft.c) !== text(saved()!.config);
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

const namesOf = (provider: string) => loginsOf(store.s!, provider).map((c) => c.name);

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
  const free = () => namesOf(props.provider).filter((n) => !inRule(rule()).includes(n));
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
  const active = () => liveOf(store.s!, props.provider)?.name ?? null;
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

type View = 'simple' | 'advanced';
const [views, setViews] = createStore<Record<string, View>>(remembered('remuda-auto-view') || {});

function CeilingInput(props: {
  name: string;
  label: string;
  value: number | undefined;
  placeholder: number;
  onSet: (c: number | null) => void;
}) {
  return (
    <label class="kept">
      <span class="aside">{props.label}</span>
      <span class="unit">
        <input
          type="number"
          min="1"
          max="100"
          aria-label={`${props.name}: ${props.label}`}
          placeholder={String(props.placeholder)}
          value={props.value ?? ''}
          onChange={(e) => {
            const c = percent(e.currentTarget.value);
            e.currentTarget.value = c === null ? '' : String(c);
            props.onSet(c);
          }}
        />
        %
      </span>
    </label>
  );
}

/** The logins TokenGauge shows with a single window of their own. */
const singles = (provider: string) =>
  loginsOf(store.s!, provider)
    .filter((c) => ownWindows(store.s!, c)?.length === 1)
    .map((c) => c.name);

function Order(props: { p: Provider; simple: boolean }) {
  const a = () => draft.c.auto?.[props.p.id] ?? {};
  const set = (fn: (a: ReturnType<typeof auto>) => void) => edit((c) => fn(auto(c, props.p.id)));
  const picked = () => a().logins ?? [];
  const rows = () => ordered(a(), namesOf(props.p.id));
  const cred = (name: string) => loginsOf(store.s!, props.p.id).find((c) => c.name === name);
  const placeholder = () => a().ceiling ?? 100;
  const windows = (name: string) => {
    const c = cred(name);
    return c ? ownWindows(store.s!, c) : null;
  };
  return (
    <ol class="order">
      <For each={rows()} fallback={<li class="aside">No stored logins</li>}>
        {(name) => {
          const i = () => picked().indexOf(name);
          return (
            <li classList={{ off: i() < 0 }}>
              <span class="aside place">{i() < 0 ? '-' : i() + 1}</span>
              <label class="check pick">
                <input
                  type="checkbox"
                  checked={i() >= 0}
                  onChange={() => set((x) => toggleLogin(x, name, !!cred(name)))}
                />
                <span class="spec">{name}</span>
                <Show when={!cred(name)}>
                  <span class="aside"> (missing)</span>
                </Show>
              </label>
              <span class="order-tools">
                <button
                  class="btn icon"
                  title={`Use ${name} earlier`}
                  disabled={i() <= 0}
                  onClick={() => set((x) => moveLogin(x, name, -1))}
                >
                  &uarr;
                </button>
                <button
                  class="btn icon"
                  title={`Use ${name} later`}
                  disabled={i() < 0 || i() === picked().length - 1}
                  onClick={() => set((x) => moveLogin(x, name, 1))}
                >
                  &darr;
                </button>
              </span>
              <span class="order-ceilings">
                <Show
                  when={!props.simple}
                  fallback={
                    <CeilingInput
                      name={name}
                      label="leave it at"
                      value={oneCeiling(a(), name, singles(props.p.id)) ?? undefined}
                      placeholder={placeholder()}
                      onSet={(c) => set((x) => setCeiling(x, ['ceilings', 'long_ceilings'], name, c))}
                    />
                  }
                >
                  <Show when={(windows(name)?.length ?? 2) > 1}>
                    <CeilingInput
                      name={name}
                      label={`${windows(name)?.[0].title.toLowerCase() ?? 'shortest window'} full at`}
                      value={a().ceilings?.[name]}
                      placeholder={placeholder()}
                      onSet={(c) => set((x) => setCeiling(x, ['ceilings'], name, c))}
                    />
                  </Show>
                  <CeilingInput
                    name={name}
                    label={`${
                      windows(name)
                        ?.slice(windows(name)!.length > 1 ? 1 : 0)
                        .map((w) => w.title.toLowerCase())
                        .join(', ') ?? 'other windows'
                    } full at`}
                    value={a().long_ceilings?.[name]}
                    placeholder={placeholder()}
                    onSet={(c) => set((x) => setCeiling(x, ['long_ceilings'], name, c))}
                  />
                </Show>
              </span>
            </li>
          );
        }}
      </For>
    </ol>
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
      (p) =>
        (p.at === at() || p.at.startsWith(`${at()}.`)) && !p.at.startsWith(`${at()}.rule[`),
    );
  const addRule = () =>
    set((x) =>
      (x.rule ??= []).push({ when: Object.keys(draft.c.schedules ?? {})[0] ?? OTHERWISE, order: [[]] }),
    );
  const hidden = () => advancedOnly(a(), namesOf(props.p.id), singles(props.p.id));
  const simple = () => !hidden().length && (views[props.p.id] ?? 'simple') === 'simple';
  const confirm = createConfirm();
  const view = (v: View) => {
    if (v === 'simple' && hidden().length) {
      if (!confirm.ask(`Simple mode drops ${hidden().join(', ')}. Click again to drop them.`, 'simple')) return;
      set((x) => simplify(x, namesOf(props.p.id), singles(props.p.id)));
    }
    confirm.disarm();
    setViews(props.p.id, v);
    remember('remuda-auto-view', { ...views });
  };
  return (
    <div class="auto-card" style={{ '--brand': brand(props.p.id) }}>
      <div class="auto-card-head">
        <h2 id={`auto-${props.p.id}`}>Switch logins automatically</h2>
        <button
          class="switch"
          role="switch"
          aria-labelledby={`auto-${props.p.id}`}
          aria-checked={!!a().enabled}
          onClick={() => set((x) => (x.enabled = !x.enabled))}
        >
          <i />
          {a().enabled ? 'On' : 'Off'}
        </button>
      </div>
      <p class="aside">
        remuda uses the first login of the list that is under its ceiling, and moves down the list when one fills up. Once
        an earlier login has room again, it goes back to it.
        <Show when={!simple() && a().balance}> Within a tier, it spends first the headroom a reset is about to wipe.</Show>
      </p>
      <Show when={status()}>{(s) => <Now s={s()} provider={props.p.id} />}</Show>
      <Show when={props.p.id === 'codex'}>
        <p class="aside">A running Codex session cannot take a new login, so auto mode waits for none to be open.</p>
      </Show>
      <div class="segmented view" role="group" aria-label="Settings">
        <button aria-pressed={simple()} onClick={() => view('simple')}>
          Simple
        </button>
        <button aria-pressed={!simple()} onClick={() => view('advanced')}>
          Advanced
        </button>
      </div>
      <Show when={confirm.text()}>
        <div class="note">{confirm.text()}</div>
      </Show>
      <div class="pickable">
        <span class="aside">
          {simple() ? 'Logins, in the order it uses them' : 'Logins it may pick when no rule is in force, in order'}
        </span>
        <Order p={props.p} simple={simple()} />
      </div>
      <Show when={!simple()}>
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
        <label class="check">
          <input
            type="checkbox"
            checked={!!a().balance}
            onChange={(e) =>
              set((x) => {
                if (e.currentTarget.checked) x.balance = true;
                else {
                  delete x.balance;
                  delete x.margin;
                }
              })
            }
          />
          Within a tier, rank logins by the headroom left before their reset, not by their place
        </label>
        <Show when={a().balance}>
          <label class="kept">
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
        </Show>
        <Show when={namesOf(props.p.id).length}>
          <label class="kept">
            <span class="aside">When every login it may pick is full</span>
            <select
              aria-label="When every login is full"
              value={a().fallback ?? ''}
              onChange={(e) => {
                const f = e.currentTarget.value;
                set((x) => {
                  if (f) x.fallback = f;
                  else {
                    delete x.fallback;
                    delete x.fallback_when_maxed;
                  }
                });
              }}
            >
              <option value="">stay where it is</option>
              <For each={namesOf(props.p.id)}>{(n) => <option value={n}>go to {n}</option>}</For>
              <Show when={a().fallback && !namesOf(props.p.id).includes(a().fallback!)}>
                <option value={a().fallback}>go to {a().fallback} (missing)</option>
              </Show>
            </select>
          </label>
          <Show when={a().fallback}>
            <label class="check">
              <input
                type="checkbox"
                checked={!!a().fallback_when_maxed}
                onChange={(e) =>
                  set((x) => {
                    if (e.currentTarget.checked) x.fallback_when_maxed = true;
                    else delete x.fallback_when_maxed;
                  })
                }
              />
              even when {a().fallback} is maxed
            </label>
          </Show>
        </Show>
        <For each={a().rule ?? []}>
          {(_, i) => <RuleCard provider={props.p.id} i={i()} count={a().rule!.length} problems={props.problems} />}
        </For>
        <button class="btn" onClick={addRule}>
          Add a rule
        </button>
      </Show>
      <Problems list={own()} />
    </div>
  );
}

const FIVE_HOURS = 300;

/** Logins with a 5-hour window, or no figures to say, and those already kept warm. */
function warmable(provider: string, warm: string[]) {
  return loginsOf(store.s!, provider)
    .filter((c) => {
      const own = ownWindows(store.s!, c);
      return !own || own.some((w) => w.minutes === FIVE_HOURS) || warm.includes(c.name);
    })
    .map((c) => c.name);
}

function KeepWarm(props: { p: Provider; problems: Problem[] }) {
  const warm = () => draft.c.warm?.[props.p.id] ?? [];
  return (
    <div class="auto-card">
      <div class="auto-card-head">
        <h2>Keep the 5-hour window open</h2>
      </div>
      <Show when={props.p.warm && warmable(props.p.id, warm()).length}>
        <p class="aside">
          The moment a picked login's window closes, remuda opens a new one with the smallest request it takes, whether
          auto mode is on or off.
        </p>
        <div class="chips warm-chips">
          <For each={warmable(props.p.id, warm())}>
            {(name) => (
              <Chip
                name={name}
                on={warm().includes(name)}
                onClick={() => edit((c) => toggleWarm(c, props.p.id, name))}
              />
            )}
          </For>
        </div>
      </Show>
      <Problems list={warmProblems(props.problems, props.p.id)} />
    </div>
  );
}

const warmProblems = (problems: Problem[], id: string) => problems.filter((p) => p.at === `warm.${id}`);

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
        <h2>Schedules</h2>
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
      const m = p.at.match(/^(?:auto|warm)\.([^.[]+)/);
      return m && !shown.has(m[1]);
    });
  });

  return (
    <section class="auto">
      <Show when={saved()}>
        <p class="aside">
          Saved in <code>{saved()!.path}</code>
        </p>
      </Show>
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
          <div class="auto-side">
            <For each={props.providers}>
              {(p) => (
                <Show when={(p.warm && warmable(p.id, draft.c.warm?.[p.id] ?? []).length) || warmProblems(problems(), p.id).length}>
                  <KeepWarm p={p} problems={problems()} />
                </Show>
              )}
            </For>
            <Schedules problems={problems()} />
          </div>
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
