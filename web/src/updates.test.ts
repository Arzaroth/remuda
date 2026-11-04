import { describe, expect, it } from 'vitest';
import { pending } from './updates';

describe('updates', () => {
  it('names the tools that have a newer release', () => {
    expect(
      pending({
        tools: [
          { name: 'remuda', available: true },
          { name: 'TokenGauge', available: false },
        ],
      }),
    ).toEqual(['remuda']);
    expect(pending({ tools: [] })).toEqual([]);
  });
});
