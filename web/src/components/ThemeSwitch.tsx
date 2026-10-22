import { createSignal } from 'solid-js';
import { Choice } from './Choice';

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
  return (
    <Choice
      label="Theme"
      value={theme()}
      onPick={pick}
      options={[
        ['auto', 'Auto'],
        ['light', 'Light'],
        ['dark', 'Dark'],
      ]}
    />
  );
}
