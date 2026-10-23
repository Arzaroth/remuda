import { createSignal } from 'solid-js';

export type Sheet = { provider?: string };

const [sheet, setSheet] = createSignal<Sheet | undefined>();

export { sheet };
export const openSheet = (s: Sheet) => setSheet(s);
export const closeSheet = () => setSheet(undefined);
