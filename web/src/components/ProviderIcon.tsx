import { brand } from '../brand';
import { ICONS } from '../icons';

/** A CLI's logo, in its colour. Decorative unless `label` names it. */
export function ProviderIcon(props: { provider: string; size?: number; label?: string; plain?: boolean }) {
  const icon = () => ICONS[props.provider];
  return (
    <svg
      class="picon"
      viewBox={icon()?.viewBox ?? '0 0 24 24'}
      width={props.size ?? 18}
      height={props.size ?? 18}
      fill="currentColor"
      role={props.label ? 'img' : undefined}
      aria-label={props.label}
      aria-hidden={props.label ? undefined : 'true'}
      style={props.plain ? undefined : { color: brand(props.provider) }}
      innerHTML={icon()?.inner ?? '<circle cx="12" cy="12" r="8"/>'}
    />
  );
}
