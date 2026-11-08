import { describe, expect, it } from 'vitest';
import { baseUrl, blocked, exports, masked, status } from './proxy';
import type { Proxy } from './types';

const proxy = (over: Partial<Proxy> = {}): Proxy => ({
  enabled: false,
  port: 8317,
  lan: false,
  host: '127.0.0.1',
  firewall: null,
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

  it('writes the shell exports each protocol reads', () => {
    expect(exports(proxy(), 'claude', 'rmd-k')).toBe(
      'export ANTHROPIC_BASE_URL="http://127.0.0.1:8317"\nexport ANTHROPIC_API_KEY="rmd-k"',
    );
    expect(exports(proxy(), 'openai', 'rmd-k')).toBe(
      'export OPENAI_BASE_URL="http://127.0.0.1:8317/v1"\nexport OPENAI_API_KEY="rmd-k"',
    );
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

  it('names the address other machines use, without claiming they reach it', () => {
    expect(status(proxy({ enabled: true, running: 8317, lan: true, host: '192.168.1.5' }))).toEqual({
      text: 'Listening on every interface, port 8317; other machines use 192.168.1.5',
      tone: 'good',
    });
    const lost = status(proxy({ enabled: true, running: 8317, lan: true, host: null }));
    expect(lost.tone).toBe('bad');
    expect(lost.text).toContain('LAN address was not found');
  });

  it('warns only when a firewall may block the lan port', () => {
    const ufw = (open: boolean | null) => ({ name: 'ufw', open, commands: ['sudo ufw allow 8317/tcp'] });
    const on = { enabled: true, running: 8317, lan: true };
    expect(blocked(proxy({ ...on, firewall: ufw(false) }))).toContain('ufw blocks port 8317');
    expect(blocked(proxy({ ...on, firewall: ufw(null) }))).toContain('cannot tell for sure');
    expect(blocked(proxy({ ...on, firewall: ufw(true) }))).toBeNull();
    expect(blocked(proxy({ ...on, firewall: null }))).toBeNull();
    expect(blocked(proxy({ ...on, lan: false, firewall: ufw(false) }))).toBeNull();
    expect(blocked(proxy({ ...on, running: null, firewall: ufw(false) }))).toBeNull();
  });
});
