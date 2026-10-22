import { ICONS } from './icons';

export const brand = (provider: string) => {
  const icon = ICONS[provider];
  return icon ? (icon.color ?? 'var(--ink)') : 'var(--muted)';
};
