import { describe, expect, it } from 'vitest';
import { toggleWarm, type Config } from './config';
import { warmLine } from './warm';

const NOW = Date.parse('2026-10-21T10:00:00Z');

describe('keep-warm', () => {
  it('says what the last round did', () => {
    expect(warmLine({ state: 'pinged', at: NOW - 120e3, model: 'claude-haiku-4-5' }, NOW).text).toContain('ago');
    expect(warmLine({ state: 'failed', at: NOW, error: 'boom' }, NOW)).toEqual({
      text: 'Kept warm: the last ping failed: boom',
      tone: 'bad',
    });
    expect(warmLine({ state: 'open', resetsAt: NOW + 3600e3 }, NOW).text).toContain('reopened');
    expect(warmLine({ state: 'waiting', why: 'stale' }, NOW).text).toBe('Kept warm: stale');
  });

  it('toggles a login and drops an emptied provider', () => {
    const c: Config = {};
    toggleWarm(c, 'claude', 'work');
    toggleWarm(c, 'claude', 'perso');
    expect(c.warm).toEqual({ claude: ['work', 'perso'] });
    toggleWarm(c, 'claude', 'work');
    toggleWarm(c, 'claude', 'perso');
    expect(c.warm).toEqual({});
  });
});
