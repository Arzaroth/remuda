import { describe, expect, it } from 'vitest';
import { MIN } from './format';
import type { Vital } from './types';
import { detail, parts, tone, worst } from './vitals';

const vital = (v: Partial<Vital>): Vital => ({ id: 'x', level: 'ok', title: 'X', detail: '', ...v });

describe('worst', () => {
  it('is the most serious level, ok when there is none', () => {
    expect(worst([])).toBe('ok');
    expect(worst([vital({ level: 'info' }), vital({ level: 'bad' }), vital({ level: 'warn' })])).toBe('bad');
    expect(worst([vital({ level: 'info' })])).toBe('info');
  });

  it('only warn and bad colour the dot', () => {
    expect(['ok', 'info', 'warn', 'bad'].map((l) => tone(l as Vital['level']))).toEqual(['', '', 'warn', 'bad']);
  });
});

describe('detail', () => {
  it('says how long ago, from now rather than when it was sent', () => {
    const v = vital({ detail: '{since}, refreshed 2 logins', since: 0 });
    expect(detail(v, 5 * MIN)).toBe('5 minutes ago, refreshed 2 logins');
    expect(detail(vital({ detail: 'on' }), 5 * MIN)).toBe('on');
  });
});

describe('parts', () => {
  it('splits out commands and links', () => {
    expect(parts('`systemctl --user restart x`')).toEqual([{ kind: 'code', text: 'systemctl --user restart x' }]);
    expect(parts('read it, then `remuda pin --allow`')).toEqual([
      { kind: 'text', text: 'read it, then ' },
      { kind: 'code', text: 'remuda pin --allow' },
    ]);
    expect(parts('install it from https://github.com/Arzaroth/TokenGauge')).toEqual([
      { kind: 'text', text: 'install it from ' },
      { kind: 'link', text: 'https://github.com/Arzaroth/TokenGauge' },
    ]);
  });
});
