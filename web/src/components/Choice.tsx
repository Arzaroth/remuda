import { For, Show } from 'solid-js';
import { Icon, type IconName } from './Icon';

export function Choice<T extends string>(props: {
  label: string;
  value: T;
  options: [T, string, IconName?][];
  onPick: (v: T) => void;
}) {
  return (
    <div class="theme" role="group" aria-label={props.label}>
      <For each={props.options}>
        {([v, name, icon]) => (
          <Show
            when={icon}
            fallback={
              <button aria-pressed={props.value === v} onClick={() => props.onPick(v)}>
                {name}
              </button>
            }
          >
            {(icon) => (
              <button
                class="icon"
                aria-pressed={props.value === v}
                aria-label={name}
                title={name}
                onClick={() => props.onPick(v)}
              >
                <Icon name={icon()} />
              </button>
            )}
          </Show>
        )}
      </For>
    </div>
  );
}
