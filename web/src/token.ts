type Channel = Pick<BroadcastChannel, 'postMessage' | 'addEventListener' | 'removeEventListener'>;

export function share(channel: Channel, token: string) {
  channel.addEventListener('message', (e) => {
    if (e.data?.ask) channel.postMessage({ token });
  });
}

export function ask(channel: Channel, wait: number): Promise<string> {
  return new Promise((done) => {
    const answer = (e: MessageEvent) => {
      if (typeof e.data?.token === 'string') finish(e.data.token);
    };
    const timer = setTimeout(() => finish(''), wait);
    function finish(token: string) {
      clearTimeout(timer);
      channel.removeEventListener('message', answer);
      done(token);
    }
    channel.addEventListener('message', answer);
    channel.postMessage({ ask: true });
  });
}
