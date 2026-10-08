import { createSignal, For } from 'solid-js';

type Theme = 'auto' | 'light' | 'dark';

export function ThemeSwitch() {
  const [theme, setTheme] = createSignal<Theme>((document.documentElement.dataset.theme as Theme) || 'auto');
  const pick = (t: Theme) => {
    if (t === 'auto') delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = t;
    try {
      localStorage.setItem('remuda-theme', t);
    } catch {}
    setTheme(t);
  };
  const names: [Theme, string][] = [
    ['auto', 'Auto'],
    ['light', 'Light'],
    ['dark', 'Dark'],
  ];
  return (
    <div class="theme" role="group" aria-label="Theme">
      <For each={names}>
        {([t, name]) => (
          <button aria-pressed={theme() === t} onClick={() => pick(t)}>
            {name}
          </button>
        )}
      </For>
    </div>
  );
}
