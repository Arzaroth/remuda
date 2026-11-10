import { plural } from './format';
import type { Banked, ResetKind } from './types';

const NAMES: Record<ResetKind, { noun: string; button: string; title: string }> = {
  session: { noun: '5-hour reset', button: 'Reset 5-hour limit', title: 'Clears the 5-hour limit now' },
  weekly: { noun: 'weekly reset', button: 'Reset weekly limits', title: 'Clears the weekly limits now' },
  full: { noun: 'full reset', button: 'Reset limits', title: 'Clears the session and weekly limits now' },
  fable: {
    noun: 'Fable reset',
    button: 'Reset Fable limit',
    title: "Resets your Fable limit, and takes this week's Fable use off your weekly limit",
  },
};

const FABLE_AND_SESSION =
  "Resets your Fable limit and your 5-hour limit, and takes this week's Fable use off your weekly limit";

export function describeBanked(r: Banked): { label: string; button: string; title: string } {
  const kinds = r.kinds ?? [];
  const only = kinds.length === 1 ? NAMES[kinds[0].kind] : undefined;
  if (only) {
    const title = kinds[0].kind === 'fable' && kinds[0].clears?.includes('five_hour') ? FABLE_AND_SESSION : only.title;
    return { label: plural(r.available, only.noun), button: only.button, title };
  }
  return {
    label: plural(r.available, 'banked reset'),
    button: 'Reset limits',
    title: kinds.length
      ? `Spends the reset Claude names next: ${kinds.map((k) => plural(k.count, NAMES[k.kind]?.noun ?? k.kind)).join(', ')}`
      : 'Clears the session and weekly limits now',
  };
}
