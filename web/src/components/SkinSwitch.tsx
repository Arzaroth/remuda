import { createSignal } from 'solid-js';
import { Choice } from './Choice';

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
  return (
    <Choice
      label="Look"
      value={skin()}
      onPick={pick}
      options={[
        ['sage', 'Sage'],
        ['instrument', 'Instrument'],
      ]}
    />
  );
}
