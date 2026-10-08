export const token = (() => {
  const fromHash = location.hash.slice(1);
  try {
    if (fromHash) sessionStorage.setItem('remuda-token', fromHash);
    return fromHash || sessionStorage.getItem('remuda-token') || '';
  } catch {
    return fromHash;
  }
})();
if (location.hash) history.replaceState(null, '', location.pathname);

export async function api<T = any>(path: string, body?: unknown): Promise<T> {
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
          const parts = (buf + chunk.value).split('\n\n');
          buf = parts.pop() ?? '';
          if (parts.some((e) => e.split('\n').includes('data: changed'))) changed();
        }
      }
    } catch {}
    await new Promise((done) => setTimeout(done, wait));
    wait = Math.min(wait * 2, 30000);
  }
}
