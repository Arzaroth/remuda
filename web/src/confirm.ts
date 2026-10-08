import { createSignal, onCleanup } from 'solid-js';

export function createConfirm() {
  const [text, setText] = createSignal<string | null>(null);
  let what = '';
  let at = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const disarm = () => {
    clearTimeout(timer);
    setText(null);
  };
  const ask = (question: string, about = '') => {
    if (text() !== null && what === about) {
      if (Date.now() - at <= 400) return false;
      disarm();
      return true;
    }
    disarm();
    what = about;
    at = Date.now();
    setText(question);
    timer = setTimeout(disarm, 4000);
    return false;
  };
  onCleanup(() => clearTimeout(timer));
  return { text, ask, disarm };
}
