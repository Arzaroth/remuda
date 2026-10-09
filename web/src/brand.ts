// TokenGauge's colours, so a provider reads the same on both. Grok's is black,
// which the page's own ink stands in for so it shows on either theme.
const BRANDS: Record<string, string> = {
  claude: '#de7356',
  codex: '#74aa9c',
  cursor: '#6c7086',
  glm: '#e85a6a',
  grok: 'var(--ink)',
  kimi: '#fe603c',
  opencode: '#5b9dd9',
};

export const brand = (provider: string) => BRANDS[provider] ?? 'var(--muted)';
