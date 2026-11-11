import { plural } from './format';
import type { Banked, ResetKind } from './types';

const NAMES: Record<ResetKind, { noun: string; button: string; title: string; idle: string }> = {
  session: {
    noun: '5-hour reset',
    button: 'Reset 5-hour limit',
    title: 'Clears the 5-hour limit now',
    idle: "There's no 5-hour usage to reset right now.",
  },
  weekly: {
    noun: 'weekly reset',
    button: 'Reset weekly limits',
    title: 'Clears the weekly limits now',
    idle: "There's no weekly usage to reset right now.",
  },
  full: {
    noun: 'full reset',
    button: 'Reset limits',
    title: 'Clears the session and weekly limits now',
    idle: "There's no usage to reset right now.",
  },
  fable: {
    noun: 'Fable reset',
    button: 'Reset Fable limit',
    title: "Resets your Fable limit, and takes this week's Fable use off your weekly limit",
    idle: "There's no Fable usage to reset right now.",
  },
};

const FABLE_AND_SESSION =
  "Resets your Fable limit and your 5-hour limit, and takes this week's Fable use off your weekly limit";

export type BankedRow = {
  kind?: ResetKind;
  label: string;
  expiresAt: string | null;
  button: string;
  title: string;
  idle?: string;
};

/** One row per kind TokenGauge reports, or a single row for a count it does not split. */
export function bankedRows(r: Banked): BankedRow[] {
  const kinds = r.kinds ?? [];
  const split =
    kinds.length > 0 && kinds.every((k) => k.kind in NAMES) && kinds.reduce((n, k) => n + k.count, 0) === r.available;
  if (!split) {
    return [
      {
        label: plural(r.available, 'banked reset'),
        expiresAt: r.expiresAt,
        button: 'Reset limits',
        title: 'Spends the reset Claude names next',
      },
    ];
  }
  return kinds.map((k) => {
    const names = NAMES[k.kind];
    return {
      kind: k.kind,
      label: plural(k.count, names.noun),
      expiresAt: k.expiresAt,
      button: names.button,
      title: k.kind === 'fable' && k.clears?.includes('five_hour') ? FABLE_AND_SESSION : names.title,
      idle: k.idle ? names.idle : undefined,
    };
  });
}
