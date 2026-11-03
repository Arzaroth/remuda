import { describe, expect, it } from 'vitest';
import { HOUR, MIN } from './format';
import { outlook, trace } from './forecast';
import type { Window } from './types';

const now = Date.parse('2026-11-03T12:00:00Z');
const win = (over: Partial<Window>): Window => ({
  title: '5 hours',
  minutes: 300,
  usedPercent: 40,
  resetsAt: new Date(now + 3 * HOUR).toISOString(),
  ...over,
});

describe('outlook', () => {
  it('says where a window ends at its recent burn', () => {
    expect(outlook(win({ forecast: { ends: 75 }, perHour: 11.66 }), now)).toEqual({
      text: 'Ends ~75% at this pace',
      tone: 'warn',
      title: 'At 12% an hour lately',
    });
    expect(outlook(win({ forecast: { ends: 35 }, perHour: 0 }), now)?.title).toBe('At 0.0% an hour lately');
  });

  it('says when a window runs out, in the alarm tone', () => {
    const o = outlook(win({ forecast: { fullAt: new Date(now + 90 * MIN).toISOString() }, perHour: 40 }), now);
    expect(o?.tone).toBe('bad');
    expect(o?.text).toMatch(/^Full .+ at this pace$/);
  });

  it('says nothing without a forecast, as from a TokenGauge before 0.54', () => {
    expect(outlook(win({}), now)).toBeNull();
    expect(outlook(win({ forecast: null }), now)).toBeNull();
    expect(outlook(win({ forecast: { fullAt: 'soon' } }), now)).toBeNull();
  });
});

describe('trace', () => {
  const opened = now - 2 * HOUR;
  const at = (minutes: number) => opened + minutes * MIN;

  it('steps from an empty window through each sample to now', () => {
    const t = trace(win({ trail: [[at(30), 10], [at(90), 30]], forecast: { ends: 70 } }), now);
    expect(t).toEqual({
      line: 'M0 100 H10 V90 H30 V70 H40 V60',
      ahead: 'M40 60 L100 30',
      now: 40,
    });
  });

  it('runs the forecast to the empty mark when the window runs out first', () => {
    const t = trace(win({ trail: [[at(60), 40]], forecast: { fullAt: new Date(now + 60 * MIN).toISOString() } }), now);
    expect(t?.ahead).toBe('M40 60 L60 0');
  });

  it('draws nothing without a trail or a window to lay it on', () => {
    expect(trace(win({}), now)).toBeNull();
    expect(trace(win({ trail: [] }), now)).toBeNull();
    expect(trace(win({ trail: [[at(30), 10]], minutes: null }), now)).toBeNull();
    expect(trace(win({ trail: [[at(30), 10]], resetsAt: null }), now)).toBeNull();
    expect(trace(win({ trail: [[at(30), 10]], resetsAt: new Date(now - MIN).toISOString() }), now)).toBeNull();
  });
});
