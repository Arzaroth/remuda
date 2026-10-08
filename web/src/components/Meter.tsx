import type { Tone } from '../format';

export function Meter(props: { fraction: number; tone: Tone }) {
  const w = () => (Math.max(0, Math.min(1, props.fraction)) * 100).toFixed(1);
  return (
    <div class={`meter ${props.tone}`}>
      <i style={{ '--w': `${w()}%` }} />
    </div>
  );
}
