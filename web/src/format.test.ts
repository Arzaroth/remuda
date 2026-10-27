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
    expect(until(0, 2 * HOUR + 5 * MIN)).toBe('2 hours and 5 minutes ago');
    expect(ago(0, 5 * MIN)).toBe('5 minutes ago');
  });
});

describe('until', () => {
  it('counts down in the two largest units', () => {
    expect(until(DAY + 10 * HOUR + 59 * MIN, 0)).toBe('in 1 day and 10 hours');
    expect(until(HOUR + 25 * MIN + 30e3, 0)).toBe('in 1 hour and 25 minutes');
    expect(until(3 * DAY + 30 * MIN, 0)).toBe('in 3 days');
    expect(until(45 * MIN, 0)).toBe('in 45 minutes');
  });
});

describe('usedTone', () => {
  it('warns from 70% and alarms from 90%', () => {
    expect([69, 70, 89, 90].map(usedTone)).toEqual(['', 'warn', 'warn', 'bad']);
  });
});
