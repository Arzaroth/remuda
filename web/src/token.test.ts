import { afterEach, describe, expect, it } from 'vitest';
import { ask, share } from './token';

const open: BroadcastChannel[] = [];
const channel = () => {
  const c = new BroadcastChannel('remuda-token-test');
  open.push(c);
  return c;
};
afterEach(() => {
  for (const c of open.splice(0)) c.close();
});

describe('token', () => {
  it('a new tab gets the token from an open one', async () => {
    share(channel(), 'secret');
    expect(await ask(channel(), 1000)).toBe('secret');
  });

  it('a new tab with no other tab open gives up empty', async () => {
    expect(await ask(channel(), 50)).toBe('');
  });

  it('a tab still asking does not answer another', async () => {
    const first = ask(channel(), 50);
    expect(await ask(channel(), 50)).toBe('');
    expect(await first).toBe('');
  });
});
