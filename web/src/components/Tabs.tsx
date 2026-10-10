import { For } from 'solid-js';
import { brand } from '../brand';
import { store } from '../state';
import { inUse } from '../usage';

export function Tabs(props: { value: string; onChange: (id: string) => void }) {
  const count = (id: string) => store.s!.credentials.filter((c) => id === 'all' || c.provider === id).length;
  const tabs = () => [{ id: 'all', name: 'All' }, ...inUse(store.s!)];
  return (
    <div class="tabs" role="group" aria-label="Logins shown">
      <For each={tabs()}>
        {(t) => (
          <button aria-pressed={props.value === t.id} onClick={() => props.onChange(t.id)}>
            {t.id !== 'all' && <span class="brand-dot" style={{ background: brand(t.id) }} />}
            {t.name}
            <span class="count">{count(t.id)}</span>
          </button>
        )}
      </For>
    </div>
  );
}
