const BRANDS: Record<string, string> = { claude: '#de7356', codex: '#74aa9c' };

export const brand = (provider: string) => BRANDS[provider] ?? 'var(--muted)';
