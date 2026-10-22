import type { Proxy } from './types';

export type Protocol = 'openai' | 'claude';

export const PROTOCOLS: { id: Protocol; name: string; format: string }[] = [
  { id: 'openai', name: 'OpenAI', format: 'OpenAI-compatible format' },
  { id: 'claude', name: 'Claude', format: 'Anthropic-compatible format' },
];

export const baseUrl = (p: Proxy, protocol: Protocol) => p.baseUrls[protocol];

const ENV: Record<Protocol, [string, string]> = {
  openai: ['OPENAI_BASE_URL', 'OPENAI_API_KEY'],
  claude: ['ANTHROPIC_BASE_URL', 'ANTHROPIC_API_KEY'],
};

export const exports = (p: Proxy, protocol: Protocol, key: string) => {
  const [url, secret] = ENV[protocol];
  return `export ${url}="${baseUrl(p, protocol)}"\nexport ${secret}="${key}"`;
};

export const masked = (key: string) => '•'.repeat(Math.min(Math.max(key.length, 8), 24));

export function status(p: Proxy): { text: string; tone: 'good' | 'bad' | 'muted' } {
  if (p.running !== null) return { text: `Listening on 127.0.0.1:${p.running}`, tone: 'good' };
  if (p.enabled && p.error) return { text: p.error, tone: 'bad' };
  if (p.enabled) return { text: 'Starting', tone: 'muted' };
  return { text: 'Off', tone: 'muted' };
}
