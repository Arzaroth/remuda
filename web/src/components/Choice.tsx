import { For } from 'solid-js';

export function Choice<T extends string>(props: {
  label: string;
  value: T;
  options: [T, string][];
  onPick: (v: T) => void;
}) {
  return (
    <div class="theme" role="group" aria-label={props.label}>
      <For each={props.options}>
        {([v, name]) => (
          <button aria-pressed={props.value === v} onClick={() => props.onPick(v)}>
            {name}
          </button>
        )}
      </For>
    </div>
  );
}
