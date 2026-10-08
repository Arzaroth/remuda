import { describe, expect, it } from 'vitest';
import { ago, DAY, HOUR, MIN, span, until, usedTone } from './format';

describe('span', () => {
  it('rounds down to the largest unit', () => {
    expect(span(30e3)).toBe('under a minute');
    expect(span(MIN)).toBe('1 minute');
    expect(span(90 * MIN)).toBe('1 hour');
    expect(span(-3 * DAY - HOUR)).toBe('3 days');
  });

  it('says which side of now a time is', () => {
    expect(until(2 * HOUR, 0)).toBe('in 2 hours');
    expect(until(0, 2 * HOUR)).toBe('2 hours ago');
    expect(ago(0, 5 * MIN)).toBe('5 minutes ago');
  });
});

describe('usedTone', () => {
  it('warns from 70% and alarms from 90%', () => {
    expect([69, 70, 89, 90].map(usedTone)).toEqual(['', 'warn', 'warn', 'bad']);
  });
});
