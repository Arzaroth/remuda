import { describe, expect, it } from 'vitest';
import { bankedRows } from './banked';

const at = '2027-03-09T07:59:00+00:00';

describe('bankedRows', () => {
  it('gives each kind its own row, button and expiry', () => {
    const rows = bankedRows({
      available: 2,
      expiresAt: '2026-12-10T22:00:00+00:00',
      kinds: [
        { kind: 'fable', count: 1, expiresAt: at, idle: true },
        { kind: 'weekly', count: 1, expiresAt: '2026-12-10T22:00:00+00:00' },
      ],
    });
    expect(rows.map((r) => [r.kind, r.label, r.button, r.expiresAt])).toEqual([
      ['fable', '1 Fable reset', 'Reset Fable limit', at],
      ['weekly', '1 weekly reset', 'Reset weekly limits', '2026-12-10T22:00:00+00:00'],
    ]);
    expect(rows[0].idle).toBe("There's no Fable usage to reset right now.");
    expect(rows[1].idle).toBeUndefined();
  });

  it('says a Fable reset clears the 5-hour limit on an account that has one', () => {
    const title = (clears?: string[]) =>
      bankedRows({ available: 3, expiresAt: null, kinds: [{ kind: 'fable', count: 3, expiresAt: null, clears }] })[0].title;
    expect(title(['five_hour', 'seven_day_overage_included'])).toBe(
      "Resets your Fable limit and your 5-hour limit, and takes this week's Fable use off your weekly limit",
    );
    expect(title(['seven_day_overage_included'])).toBe(
      "Resets your Fable limit, and takes this week's Fable use off your weekly limit",
    );
    expect(title()).toBe("Resets your Fable limit, and takes this week's Fable use off your weekly limit");
  });

  it('keeps one plain row when the count is not split, or the split does not add up', () => {
    const plain = (r: Parameters<typeof bankedRows>[0]) => bankedRows(r).map((x) => [x.kind, x.label, x.button]);
    expect(plain({ available: 1, expiresAt: null })).toEqual([[undefined, '1 banked reset', 'Reset limits']]);
    expect(plain({ available: 2, expiresAt: null, kinds: [{ kind: 'fable', count: 1, expiresAt: null }] })).toEqual([
      [undefined, '2 banked resets', 'Reset limits'],
    ]);
  });
});
