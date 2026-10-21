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

export type Window = { title: string; minutes?: number | null; usedPercent: number; resetsAt: string | null };

export type Usage = {
  stale?: boolean;
  staleReason?: string | null;
  error?: string | null;
  credentialState?: string | null;
  planWeight?: number | null;
  plan?: string | null;
  windows: Window[];
  bankedResets?: { available: number; expiresAt: string | null } | null;
  accounts?: Record<string, Usage>;
};

export type Shown = Usage & { passive?: boolean; missing?: boolean; behind?: boolean };

export type Health = {
  timer: 'enabled' | 'disabled' | 'masked' | 'absent';
  lastRefresh: { at: number; refreshed: string[]; problems: string[] } | null;
  tokengauge: boolean;
  switchedAt: Record<string, number>;
};

export type Part = { name: string; active: boolean; window: Window; weight: number };

// A window added up across a CLI's logins, as the server sends it.
export type Combined = {
  title: string;
  weighted: boolean;
  used: number;
  of: number;
  pooled: number;
  parts: Part[];
  widths: number[];
  leftOut: string[];
};

export type ProviderShown = {
  usage: Shown | null;
  accounts: Record<string, Shown>;
  weighable: boolean;
  weighted: Combined[];
  absolute: Combined[];
};

export type AutoSeen = {
  name: string;
  tier: number | null;
  score: number | null;
  usable: boolean;
  maxed: boolean;
  note: string;
};

export type AutoStatus = {
  enabled: boolean;
  at: number;
  decision: {
    rule: string | null;
    logins: AutoSeen[];
    action: { kind: 'stay' | 'switch' | 'hold'; to?: string; why: string };
  };
  switched?: { at: number; to: string; why: string };
  error?: string;
};

export type State = {
  store: string;
  providers: Provider[];
  credentials: Credential[];
  live: Live[];
  health: Health;
  usage: { updatedAt: number | null; providers: Record<string, Usage> } | null;
  shown: Record<string, ProviderShown>;
  auto?: Record<string, AutoStatus | { enabled: false } | string>;
};
