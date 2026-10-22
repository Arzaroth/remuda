import { describe, expect, it } from 'vitest';
import { href, parse, type Route, tokenIn } from './hash';

describe('routes', () => {
  it('reads back every route it writes', () => {
    const all: Route[] = [
      { page: 'overview' },
      { page: 'cli', id: 'claude', tab: 'logins' },
      { page: 'cli', id: 'codex', tab: 'automation' },
      { page: 'proxy' },
      { page: 'log' },
      { page: 'status' },
    ];
    for (const r of all) expect(parse(href(r))).toEqual(r);
  });

  it('falls back to the overview', () => {
    expect(parse('')).toEqual({ page: 'overview' });
    expect(parse('#/cli/')).toEqual({ page: 'overview' });
    expect(parse('#/nowhere')).toEqual({ page: 'overview' });
    expect(parse('#abc_DEF-123')).toEqual({ page: 'overview' });
  });

  it('opens an unknown tab on the logins', () => {
    expect(parse('#/cli/grok/other')).toEqual({ page: 'cli', id: 'grok', tab: 'logins' });
  });
});

describe('tokenIn', () => {
  it('takes a token hash and leaves a route alone', () => {
    expect(tokenIn('#abc_DEF-123')).toBe('abc_DEF-123');
    expect(tokenIn('#/cli/claude')).toBe('');
    expect(tokenIn('#/')).toBe('');
    expect(tokenIn('')).toBe('');
  });
});
