type Channel = Pick<BroadcastChannel, 'postMessage' | 'addEventListener' | 'removeEventListener'>;

export function share(channel: Channel, token: string) {
  channel.addEventListener('message', (e) => {
    if (e.data?.ask) channel.postMessage({ token });
  });
}

// A tab left over from an earlier server still answers with its token, so
// each answer is tried before it is taken.
export function ask(channel: Channel, wait: number, works: (token: string) => Promise<boolean>): Promise<string> {
  return new Promise((done) => {
    let over = false;
    const answer = async (e: MessageEvent) => {
      const token = e.data?.token;
      if (typeof token !== 'string') return;
      if (await works(token)) finish(token);
    };
    const timer = setTimeout(() => finish(''), wait);
    function finish(token: string) {
      if (over) return;
      over = true;
      clearTimeout(timer);
      channel.removeEventListener('message', answer);
      done(token);
    }
    channel.addEventListener('message', answer);
    channel.postMessage({ ask: true });
  });
}
