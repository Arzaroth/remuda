import { createSignal } from 'solid-js';
import { href, parse, type Route } from './hash';

export { href, type Route };

const [route, setRoute] = createSignal<Route>(parse(location.hash));
window.addEventListener('hashchange', () => setRoute(parse(location.hash)));

export { route };
export const go = (r: Route) => {
  location.hash = href(r);
};
