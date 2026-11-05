import { afterEach, describe, expect, it } from 'vitest';
import { ask, share } from './token';

const open: BroadcastChannel[] = [];
const yes = async () => true;
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
    expect(await ask(channel(), 1000, yes)).toBe('secret');
  });

  it('a new tab with no other tab open gives up empty', async () => {
    expect(await ask(channel(), 50, yes)).toBe('');
  });

  it('a tab still asking does not answer another', async () => {
    const first = ask(channel(), 50, yes);
    expect(await ask(channel(), 50, yes)).toBe('');
    expect(await first).toBe('');
  });

  it('a stale token from a tab of an earlier server is passed over for a working one', async () => {
    share(channel(), 'stale');
    share(channel(), 'fresh');
    expect(await ask(channel(), 1000, async (t) => t === 'fresh')).toBe('fresh');
  });

  it('only stale answers leave the new tab without a token', async () => {
    share(channel(), 'stale');
    expect(await ask(channel(), 50, async () => false)).toBe('');
  });

  it('a check that failed once does not stop the same token from another tab', async () => {
    share(channel(), 'secret');
    share(channel(), 'secret');
    let checks = 0;
    expect(await ask(channel(), 1000, async () => ++checks > 1)).toBe('secret');
  });
});
