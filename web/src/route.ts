import { createSignal } from 'solid-js';

export type Route =
  | { page: 'overview' }
  | { page: 'cli'; id: string; tab: 'logins' | 'automation' }
  | { page: 'proxy' }
  | { page: 'status' };

function parse(hash: string): Route {
  const [page, id, tab] = hash.replace(/^#\/?/, '').split('/');
  if (page === 'cli' && id) return { page: 'cli', id, tab: tab === 'automation' ? 'automation' : 'logins' };
  if (page === 'proxy' || page === 'status') return { page };
  return { page: 'overview' };
}

export function href(r: Route): string {
  switch (r.page) {
    case 'cli':
      return `#/cli/${r.id}${r.tab === 'automation' ? '/automation' : ''}`;
    case 'overview':
      return '#/';
    default:
      return `#/${r.page}`;
  }
}

const [route, setRoute] = createSignal<Route>(parse(location.hash.startsWith('#/') ? location.hash : ''));
window.addEventListener('hashchange', () => setRoute(parse(location.hash)));

export { route };
export const go = (r: Route) => {
  location.hash = href(r);
};
