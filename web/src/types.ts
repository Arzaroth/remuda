export type Provider = { id: string; name: string };

export type Credential = {
  id: string;
  provider: string;
  name: string;
  label: string | null;
  email: string;
  plan: string | null;
  active: boolean;
  expiresAt: number | null;
  refreshTokenExpiresAt: number | null;
  verified: boolean;
};

export type Live = {
  id: string;
  provider: string;
  state: 'signed_out' | 'foreign' | 'unreadable' | 'stored' | 'unstored';
  what?: string;
  error?: string;
  name?: string;
  confirmed?: boolean;
  email?: string | null;
};

export type Window = { title: string; usedPercent: number; resetsAt: string | null };

export type Usage = {
  stale?: boolean;
  staleReason?: string | null;
  error?: string | null;
  credentialState?: string | null;
  planWeight?: number | null;
  windows: Window[];
  accounts?: Record<string, Usage>;
};

export type Shown = Usage & { passive?: boolean; missing?: boolean; behind?: boolean };

export type Health = {
  timer: 'enabled' | 'disabled' | 'masked' | 'absent';
  lastRefresh: { at: number; refreshed: string[]; problems: string[] } | null;
  tokengauge: boolean;
  switchedAt: Record<string, number>;
};

export type State = {
  store: string;
  providers: Provider[];
  credentials: Credential[];
  live: Live[];
  health: Health;
  usage: { updatedAt: number | null; providers: Record<string, Usage> } | null;
};
