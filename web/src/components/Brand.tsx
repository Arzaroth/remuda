import { brand } from '../brand';

type Mark = { read: string; d: string; turn?: number };

const C = 'M31.1 17.6 A11 11 0 1 0 31.1 34.4';

const MARKS: Record<string, Mark> = {
  claude: { read: 'Bar C', d: `M12 9 H36 ${C}` },
  codex: { read: 'Rocking X', d: 'M16 10 L32 30 M32 10 L16 30 M9 36 Q24 46 39 36' },
  grok: { read: 'Lazy G', d: 'M31.1 17.6 A11 11 0 1 0 35 26 H27', turn: 90 },
  kimi: { read: 'Circle K', d: 'M24 5 A19 19 0 1 1 23.9 5 M19 14 V34 M30 14 L19.5 24.5 L30 34' },
  glm: { read: 'Diamond Z', d: 'M24 4 L44 24 L24 44 L4 24 Z M18 18 H30 L18 30 H30' },
  opencode: { read: 'O Bar', d: 'M24 9 A10 10 0 1 1 23.9 9 M12 38 H36' },
  cursor: { read: 'Box C', d: `M8 8 H40 V40 H8 Z M28.7 18.7 A7.5 7.5 0 1 0 28.7 29.3` },
};

/** A CLI's cattle brand, burned in its colour; `name` is what it is read as. */
export function Brand(props: { provider: string; name?: string; size?: number; scorched?: boolean; ghost?: boolean }) {
  const mark = () => MARKS[props.provider];
  const title = () => `${mark()?.read ?? props.provider}, ${props.name ?? props.provider}'s brand`;
  return (
    <svg
      class="mark"
      classList={{ scorched: props.scorched, ghost: props.ghost }}
      viewBox="0 0 48 48"
      width={props.size ?? 20}
      height={props.size ?? 20}
      role={props.ghost ? undefined : 'img'}
      aria-hidden={props.ghost ? 'true' : undefined}
      aria-label={props.ghost ? undefined : title()}
      style={{ color: brand(props.provider) }}
    >
      {!props.ghost && <title>{title()}</title>}
      <path d={mark()?.d ?? 'M14 14 H34 V34 H14 Z'} transform={mark()?.turn ? `rotate(${mark()!.turn} 24 26)` : undefined} />
    </svg>
  );
}

/** The filter that roughens a scorched mark's edges, rendered once per page. */
export function Iron() {
  return (
    <svg width="0" height="0" aria-hidden="true" style={{ position: 'absolute' }}>
      <filter id="iron" x="-10%" y="-10%" width="120%" height="120%">
        <feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" seed="7" />
        <feDisplacementMap in="SourceGraphic" scale="2.2" />
      </filter>
    </svg>
  );
}
