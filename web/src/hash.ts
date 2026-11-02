export type Route =
  | { page: 'overview' }
  | { page: 'cli'; id: string; tab: 'logins' | 'automation' }
  | { page: 'usage'; tab: 'spend' | 'requests' }
  | { page: 'proxy' }
  | { page: 'log' }
  | { page: 'status' }
  | { page: 'versions' };

export const isRoute = (hash: string) => hash.startsWith('#/');

export const tokenIn = (hash: string) => (isRoute(hash) ? '' : hash.slice(1));

export function parse(hash: string): Route {
  if (!isRoute(hash)) return { page: 'overview' };
  const [page, id, tab] = hash.slice(2).split('/');
  if (page === 'cli' && id) return { page: 'cli', id, tab: tab === 'automation' ? 'automation' : 'logins' };
  if (page === 'usage') return { page, tab: id === 'requests' ? 'requests' : 'spend' };
  if (page === 'proxy' || page === 'log' || page === 'status' || page === 'versions') return { page };
  return { page: 'overview' };
}

export function href(r: Route): string {
  switch (r.page) {
    case 'cli':
      return `#/cli/${r.id}${r.tab === 'automation' ? '/automation' : ''}`;
    case 'usage':
      return r.tab === 'requests' ? '#/usage/requests' : '#/usage';
    case 'overview':
      return '#/';
    default:
      return `#/${r.page}`;
  }
}
