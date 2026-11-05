import { tokenIn } from './hash';
import { frames, isChange } from './sse';
import { ask, share } from './token';

const fromHash = tokenIn(location.hash);
if (fromHash) history.replaceState(null, '', location.pathname);

function keep(t: string) {
  try {
    sessionStorage.setItem('remuda-token', t);
  } catch {}
}

function kept() {
  try {
    return sessionStorage.getItem('remuda-token') ?? '';
  } catch {
    return '';
  }
}

let token = fromHash || kept();
const works = (t: string) =>
  fetch('/api/token', { headers: { 'X-Remuda-Token': t } }).then(
    (r) => r.ok,
    () => false,
  );
const channel = new BroadcastChannel('remuda-token');
const ready = (token ? Promise.resolve() : ask(channel, 1000, works).then((t) => (token = t))).then(() => {
  if (!token) return;
  keep(token);
  share(channel, token);
});

export async function api<T = any>(path: string, body?: unknown): Promise<T> {
  await ready;
  const init: RequestInit & { headers: Record<string, string> } = { headers: { 'X-Remuda-Token': token } };
  if (body !== undefined) {
    init.method = 'POST';
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  const r = await fetch(path, init);
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(data.error || r.statusText);
  return data;
}

export async function follow(changed: () => void) {
  await ready;
  let wait = 1000;
  let again = false;
  for (;;) {
    try {
      const r = await fetch('/api/events', { headers: { 'X-Remuda-Token': token } });
      if (r.status === 401) return;
      if (r.ok && r.body) {
        wait = 1000;
        if (again) changed();
        again = true;
        const events = r.body.pipeThrough(new TextDecoderStream()).getReader();
        for (let buf = '', chunk; !(chunk = await events.read()).done; ) {
          const { whole, rest } = frames(buf + chunk.value);
          buf = rest;
          if (whole.some(isChange)) changed();
        }
      }
    } catch {}
    await new Promise((done) => setTimeout(done, wait));
    wait = Math.min(wait * 2, 30000);
  }
}
