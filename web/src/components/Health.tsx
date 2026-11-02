import { createSignal, For, Show } from 'solid-js';
import { plural } from '../format';
import { now } from '../state';
import type { State, Vital } from '../types';
import { detail, parts, tone, worst } from '../vitals';

export function healthTone(s: State): '' | 'warn' | 'bad' {
  return tone(worst(s.health.vitals));
}

function Text(props: { text: string }) {
  return (
    <For each={parts(props.text)}>
      {(p) =>
        p.kind === 'code' ? (
          <code>{p.text}</code>
        ) : p.kind === 'link' ? (
          <a href={p.text} target="_blank" rel="noopener noreferrer">
            {p.text}
          </a>
        ) : (
          p.text
        )
      }
    </For>
  );
}

function Row(props: { v: Vital }) {
  const [open, setOpen] = createSignal(false);
  const items = () => props.v.items ?? [];
  const id = () => `vital-${props.v.id}`;
  return (
    <div class="vital">
      <span class={`dot ${props.v.level === 'ok' ? 'good' : tone(props.v.level)}`} />
      <span class="sr">{props.v.level}</span>
      <div class="vital-body">
        <div>
          <b>{props.v.title}</b> <Text text={detail(props.v, now())} />
          <Show when={items().length}>
            {' '}
            <button class="linkish" aria-expanded={open()} aria-controls={id()} onClick={() => setOpen(!open())}>
              {plural(items().length, 'detail')}
            </button>
          </Show>
        </div>
        <Show when={props.v.fix}>
          {(fix) => (
            <div class="vital-fix">
              <Text text={fix()} />
            </div>
          )}
        </Show>
        <Show when={items().length}>
          <ul class="problems" id={id()} hidden={!open()}>
            <For each={items()}>
              {(item) => (
                <li>
                  <Text text={item} />
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
    </div>
  );
}

export function Health(props: { state: State }) {
  return (
    <div class="health">
      <div class="health-line">
        <For each={props.state.health.vitals}>{(v) => <Row v={v} />}</For>
        <div>
          <span>
            Store <code>{props.state.store}</code>
          </span>
        </div>
      </div>
    </div>
  );
}
