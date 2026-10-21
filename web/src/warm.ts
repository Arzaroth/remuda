import { ago, until } from './format';
import type { WarmStatus } from './types';

/** What a login card says of keep-warm, from the server's last round. */
export function warmLine(s: WarmStatus, now: number): { text: string; tone: '' | 'bad' } {
  switch (s.state) {
    case 'pinged':
      return { text: `Kept warm: window opened ${ago(s.at, now)}`, tone: '' };
    case 'failed':
      return { text: `Kept warm: the last ping failed: ${s.error}`, tone: 'bad' };
    case 'open':
      return {
        text: `Kept warm: window open, reopened ${until(s.resetsAt, now)}`,
        tone: '',
      };
    case 'waiting':
      return { text: `Kept warm: ${s.why}`, tone: '' };
  }
}
