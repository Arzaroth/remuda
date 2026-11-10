import { describe, expect, it } from 'vitest';
import { describeBanked } from './banked';

describe('describeBanked', () => {
  it('names a single kind', () => {
    const fable = describeBanked({
      available: 1,
      expiresAt: '2027-03-09T07:59:00+00:00',
      kinds: [{ kind: 'fable', count: 1, expiresAt: '2027-03-09T07:59:00+00:00' }],
    });
    expect(fable.label).toBe('1 Fable reset');
    expect(fable.button).toBe('Reset Fable limit');
    expect(fable.title).toContain('Fable use off your weekly limit');
    expect(describeBanked({ available: 2, expiresAt: null, kinds: [{ kind: 'weekly', count: 2, expiresAt: null }] }).label).toBe(
      '2 weekly resets',
    );
  });

  it('keeps the plain count for a mix or an unsplit snapshot', () => {
    const mix = describeBanked({
      available: 3,
      expiresAt: null,
      kinds: [
        { kind: 'fable', count: 1, expiresAt: null },
        { kind: 'session', count: 2, expiresAt: null },
      ],
    });
    expect(mix.label).toBe('3 banked resets');
    expect(mix.title).toBe('Spends the reset Claude names next: 1 Fable reset, 2 5-hour resets');
    expect(describeBanked({ available: 1, expiresAt: null }).label).toBe('1 banked reset');
  });
});
