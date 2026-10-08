import type { JSX } from 'solid-js';

const PATHS: Record<IconName, () => JSX.Element> = {
  refresh: () => [<path d="M20 11a8 8 0 1 0-2.3 5.7" />, <path d="M20 4v7h-7" />],
  label: () => [<path d="M3 12V4h8l10 10-8 8z" />, <circle cx="7.5" cy="8.5" r="1.2" />],
  rename: () => [<path d="M4 20h4L19 9l-4-4L4 16z" />, <path d="m13.5 6.5 4 4" />],
  remove: () => [<path d="M4 7h16" />, <path d="M9 7V4h6v3" />, <path d="M6 7l1 13h10l1-13" />],
  save: () => <path d="m5 12.5 4.5 4.5L19 7" />,
};

export type IconName = 'refresh' | 'label' | 'rename' | 'remove' | 'save';

export function Icon(props: { name: IconName }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      {PATHS[props.name]()}
    </svg>
  );
}
