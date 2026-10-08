export function remembered<T>(key: string): T | null {
  try {
    return JSON.parse(localStorage.getItem(key) ?? 'null') ?? null;
  } catch {
    return null;
  }
}

export function remember(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {}
}
