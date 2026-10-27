import { describe, expect, it } from 'vitest';
import {
  auto,
  canonical,
  type Config,
  dropFromRule,
  headline,
  hours,
  minutes,
  percent,
  problemsAt,
  renameSchedule,
  scheduleName,
  scheduleUses,
  span,
  type Status,
} from './config';

const sample = (): Config => ({
  schedules: { workday: { days: ['mon', 'fri'], hours: '09:00-18:00' } },
  auto: {
    claude: {
      enabled: true,
      rule: [
        { when: 'workday', order: [['work'], ['perso']] },
        { when: 'otherwise', order: [['perso'], ['work']], reserve: { work: 'workday' } },
      ],
    },
  },
});

describe('spans', () => {
  it('reads and writes what the server does', () => {
    expect(minutes('1h30m')).toBe(90);
    expect(minutes('90s')).toBe(2);
    expect(minutes(undefined)).toBe(30);
    expect(span(90)).toBe('1h30m');
    expect(span(60)).toBe('1h');
    expect(span(0)).toBe('0m');
  });
});

describe('hours', () => {
  it('splits a range for the time inputs, midnight as 00:00', () => {
    expect(hours('9:00-18:00')).toEqual({ start: '09:00', end: '18:00' });
    expect(hours('18:00-24:00')).toEqual({ start: '18:00', end: '00:00' });
    expect(hours(undefined)).toBeNull();
  });
});

describe('canonical', () => {
  it('ignores key order and what the server leaves out', () => {
    const a = sample();
    const b = sample();
    b.auto!.claude = { rule: b.auto!.claude.rule, enabled: true, logins: [] };
    b.auto!.claude.rule![0].reserve = {};
    expect(canonical(a)).toBe(canonical(b));
    b.auto!.claude.enabled = false;
    expect(canonical(a)).not.toBe(canonical(b));
  });

  it('takes a login ceilings table emptied on the page for none', () => {
    expect(canonical({ auto: { claude: { ceilings: {} } } })).toBe(canonical({ auto: { claude: {} } }));
  });

  it('takes a long ceilings table emptied on the page for none', () => {
    expect(canonical({ auto: { claude: { long_ceilings: {} } } })).toBe(canonical({ auto: { claude: {} } }));
  });

  it('keeps an every-day, all-day schedule, which the server keeps', () => {
    expect(canonical({ schedules: { always: {} } })).not.toBe(canonical({}));
    expect(canonical({ schedules: {} })).toBe(canonical({}));
  });

  it('keeps an empty tier, which means something', () => {
    const a = sample();
    const b = sample();
    b.auto!.claude.rule![0].order.push([]);
    expect(canonical(a)).not.toBe(canonical(b));
  });
});

describe('schedules', () => {
  it('a rename follows into every rule and reservation', () => {
    const c = sample();
    renameSchedule(c, 'workday', 'weekdays');
    expect(Object.keys(c.schedules!)).toEqual(['weekdays']);
    expect(c.auto!.claude.rule![0].when).toBe('weekdays');
    expect(c.auto!.claude.rule![1].reserve).toEqual({ work: 'weekdays' });
  });

  it('a rename onto a taken name does nothing', () => {
    const c = sample();
    c.schedules!.nights = {};
    renameSchedule(c, 'workday', 'nights');
    expect(Object.keys(c.schedules!)).toEqual(['workday', 'nights']);
  });

  it('names what uses a schedule', () => {
    expect(scheduleUses(sample(), 'workday')).toEqual(['claude rule 1', 'claude rule 2']);
    expect(scheduleUses(sample(), 'nights')).toEqual([]);
  });

  it('refuses a name the server would', () => {
    expect(scheduleName(sample(), 'nights')).toBeNull();
    expect(scheduleName(sample(), 'workday')).toMatch(/has that name/);
    expect(scheduleName(sample(), 'otherwise')).toMatch(/catch-all/);
    expect(scheduleName(sample(), 'late nights')).toMatch(/letters/);
  });
});

describe('rules', () => {
  it('a login taken out of a rule leaves its tier and reservation', () => {
    const r = sample().auto!.claude.rule![1];
    dropFromRule(r, 'work');
    expect(r.order).toEqual([['perso'], []]);
    expect(r.reserve).toEqual({});
  });

  it('a provider with no section gets one', () => {
    const c: Config = {};
    auto(c, 'codex').enabled = true;
    expect(c).toEqual({ auto: { codex: { enabled: true } } });
  });
});

describe('problemsAt', () => {
  it('splits problems by where they are, a rule apart from its provider', () => {
    const list = [
      { at: 'auto.claude.logins', message: 'a' },
      { at: 'auto.claude.rule[1].when', message: 'b' },
      { at: 'auto.claudette', message: 'c' },
    ];
    expect(problemsAt(list, 'auto.claude').map((p) => p.message)).toEqual(['a', 'b']);
    expect(problemsAt(list, 'auto.claude.rule[1]').map((p) => p.message)).toEqual(['b']);
  });
});

describe('headline', () => {
  const status = (kind: 'stay' | 'switch' | 'hold', more: Partial<Status> = {}): Status => ({
    enabled: true,
    at: 5,
    decision: { rule: null, logins: [], action: { kind, to: 'work', why: 'because' } },
    ...more,
  });

  it('says what the round came to', () => {
    expect(headline(status('stay'), 'perso')).toEqual({ text: 'Stays on perso', tone: 'good' });
    expect(headline(status('hold'), 'perso')).toEqual({ text: 'Holding', tone: 'warn' });
    expect(headline(status('switch'), 'perso').text).toBe('Switching to work');
    expect(headline(status('switch', { switched: { at: 5, to: 'work', why: '' } }), 'work').text).toBe(
      'Switched to work',
    );
    expect(headline(status('switch', { error: 'no' }), 'perso')).toEqual({
      text: 'Could not switch to work',
      tone: 'bad',
    });
  });

  it('says what it would do when off', () => {
    expect(headline(status('switch', { enabled: false }), 'perso').text).toBe('Off; it would switch to work');
    expect(headline(status('stay', { enabled: false }), 'perso').text).toBe('Off');
  });
});

describe('percent', () => {
  it('reads a ceiling typed into a field', () => {
    expect(percent('93')).toBe(93);
    expect(percent('92.6')).toBe(93);
    expect(percent('0')).toBe(1);
    expect(percent('250')).toBe(100);
    expect(percent('')).toBeNull();
    expect(percent('abc')).toBeNull();
  });
});
