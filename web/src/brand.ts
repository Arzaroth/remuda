import { ICONS } from './icons';

const ink = (color: string | null | undefined, known: boolean) => (known ? (color ?? 'var(--ink)') : 'var(--muted)');

export const brand = (provider: string) => ink(ICONS[provider]?.color, provider in ICONS);

export const markColor = (provider: string) => ink(ICONS[provider]?.mark, provider in ICONS);
