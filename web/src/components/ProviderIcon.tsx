import { brand } from '../brand';
import { ICONS } from '../icons';

export function ProviderIcon(props: { provider: string; size?: number }) {
  const icon = () => ICONS[props.provider];
  return (
    <svg
      class="picon"
      viewBox={icon()?.viewBox ?? '0 0 24 24'}
      width={props.size ?? 18}
      height={props.size ?? 18}
      fill="currentColor"
      aria-hidden="true"
      style={{ color: brand(props.provider) }}
      innerHTML={icon()?.inner ?? '<circle cx="12" cy="12" r="8"/>'}
    />
  );
}
