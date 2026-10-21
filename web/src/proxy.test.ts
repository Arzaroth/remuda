import { describe, expect, it } from 'vitest';
import { baseUrl, masked, status } from './proxy';
import type { Proxy } from './types';

const proxy = (over: Partial<Proxy> = {}): Proxy => ({
  enabled: false,
  port: 8317,
  running: null,
  error: null,
  baseUrls: { openai: 'http://127.0.0.1:8317/v1', claude: 'http://127.0.0.1:8317' },
  keys: [],
  routes: [],
  ...over,
});

describe('proxy', () => {
  it('shows one base URL per protocol', () => {
    expect(baseUrl(proxy(), 'openai')).toBe('http://127.0.0.1:8317/v1');
    expect(baseUrl(proxy(), 'claude')).toBe('http://127.0.0.1:8317');
  });

  it('masks a key without giving its length away exactly', () => {
    expect(masked('abc')).toHaveLength(8);
    expect(masked('x'.repeat(48))).toHaveLength(24);
  });

  it('says whether it listens', () => {
    expect(status(proxy())).toEqual({ text: 'Off', tone: 'muted' });
    expect(status(proxy({ enabled: true, running: 8317 })).tone).toBe('good');
    expect(status(proxy({ enabled: true, error: 'cannot listen' }))).toEqual({ text: 'cannot listen', tone: 'bad' });
  });
});
