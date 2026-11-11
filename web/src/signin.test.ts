import { describe, expect, it } from 'vitest';
import { answers, heading, seeded, type Field } from './signin';

const region: Field = { name: 'region', label: 'the region', choices: ['global', 'cn'] };
const management: Field = {
  name: 'management',
  label: 'the management key, which the account balance needs',
  choices: [],
};

describe('a sign-in asking for more than the key', () => {
  it('starts every choice on its default and every text empty', () => {
    expect(seeded([region, management], undefined)).toEqual({ region: 'global', management: '' });
    expect(seeded([], undefined)).toEqual({});
  });

  it('signs a login in again on the region it was stored with, never on a secret', () => {
    expect(seeded([region, management], { region: 'cn', management: 'mg-1' })).toEqual({
      region: 'cn',
      management: '',
    });
    expect(seeded([region], { region: 'eu' })).toEqual({ region: 'global' });
    expect(seeded([region], { region: null })).toEqual({ region: 'global' });
  });

  it('sends only the fields it asked for, trimmed, a blank choice on its default', () => {
    expect(answers([region, management], { region: 'cn', management: ' mg-1 ', other: 'x' })).toEqual({
      region: 'cn',
      management: 'mg-1',
    });
    expect(answers([region, management], { region: ' ' })).toEqual({ region: 'global', management: '' });
    expect(answers([], { region: 'cn' })).toEqual({});
  });

  it('heads a field with its label, capitalised and without the sentence around it', () => {
    expect(heading(region)).toBe('Region');
    expect(heading(management)).toBe('Management key');
  });
});
