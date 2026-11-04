import { createSignal } from 'solid-js';

export type Checked = { tools: { name: string; available: boolean }[] };

const [updates, setUpdates] = createSignal<string[]>([]);
export { updates };

// The tools with a newer release than the one installed, by name.
export const pending = (v: Checked) => v.tools.filter((t) => t.available).map((t) => t.name);

export const noteVersions = (v: Checked) => setUpdates(pending(v));
