import { createSignal, For } from 'solid-js';

type Skin = 'sage' | 'instrument';

export function SkinSwitch() {
  const [skin, setSkin] = createSignal<Skin>((document.documentElement.dataset.skin as Skin) || 'sage');
  const pick = (k: Skin) => {
    document.documentElement.dataset.skin = k;
    try {
      localStorage.setItem('remuda-skin', k);
    } catch {}
    setSkin(k);
  };
  const names: [Skin, string][] = [
    ['sage', 'Sage'],
    ['instrument', 'Instrument'],
  ];
  return (
    <div class="theme" role="group" aria-label="Look">
      <For each={names}>
        {([k, name]) => (
          <button aria-pressed={skin() === k} onClick={() => pick(k)}>
            {name}
          </button>
        )}
      </For>
    </div>
  );
}
