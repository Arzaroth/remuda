import { createSignal } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { api } from './api';
import type { State } from './types';

const [now, setNow] = createSignal(Date.now());
export { now };
export const tick = () => setNow(Date.now());

const [store, setStore] = createStore<{ s: State | null; error: string | null; loaded: boolean }>({
  s: null,
  error: null,
  loaded: false,
});
export { store };

export function keyed(s: State): State {
  return {
    ...s,
    credentials: s.credentials.map((c) => ({ ...c, id: `${c.provider}/${c.name}` })),
    live: s.live.map((l) => ({ ...l, id: l.provider })),
  };
}

let loads = 0;
export async function load() {
  const seq = ++loads;
  try {
    const s = await api<State>('/api/state');
    if (seq !== loads) return;
    document.body.classList.toggle('fresh', store.loaded);
    tick();
    setStore('s', reconcile(keyed(s)));
    setStore({ error: null, loaded: true });
  } catch (e) {
    if (seq !== loads) return;
    setStore('error', (e as Error).message);
  }
}

const [toastState, setToast] = createSignal<{ message: string; error: boolean; show: boolean }>({
  message: '',
  error: false,
  show: false,
});
export { toastState };
let toastTimer: ReturnType<typeof setTimeout> | undefined;
export function toast(message: string, error = false) {
  setToast({ message, error, show: true });
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => setToast((t) => ({ ...t, show: false })), error ? 7000 : 3500);
}

export async function act(path: string, body: unknown) {
  try {
    const r = await api(path, body);
    if (r.message) toast(r.message);
  } catch (e) {
    toast((e as Error).message, true);
  }
  await load();
}

export function reload() {
  const busy =
    document.querySelector('#providers .edit') ||
    document.activeElement?.closest?.('#providers') ||
    document.querySelector('#providers [data-armed]');
  if (!busy) load();
}
