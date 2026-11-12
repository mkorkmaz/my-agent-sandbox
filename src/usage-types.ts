// Frozen P03 wire contract. Additions use serde camelCase in Rust.
export type LineageKind = 'primary' | 'subagent' | 'unknown';
export type RelationStatus = 'root' | 'resolved' | 'orphan' | 'invalid' | 'unknown';
export type Coverage = 'complete' | 'partial' | 'unknown';
export type SessionState = 'active' | 'archived' | 'deleted';
export type TurnStatus = string; // Known: in-progress, complete, interrupted; retain legacy values.

export interface Lineage {
  kind: LineageKind;
  parentSessionId: string | null;
  agentPath: string | null;
  nickname: string | null;
  sourceVersion: string | null;
  lineageVersion: 1;
}

export interface RequestSource {
  guestNamespace: string;
  modelProvider: string;
  originThreadId: string;
  originTurnId: string | null;
  rootTurnId: string | null;
  responseId: string;
}

export interface Tokens {
  input: number; cached: number; cacheWrite: number; output: number; reasoning: number;
  write12h: number | null; cacheKnown: boolean;
  cachedKnown: boolean | null; cacheWriteKnown: boolean | null;
}
export interface RateSnapshot {
  date: string; source: string; input: number; cached: number;
  write30m: number; write12h: number; output: number;
  tier: string; tierAssumed: boolean; longContext: boolean;
}
export interface RateCorrection {
  previousRates: RateSnapshot; previousCostUsd: number | null; reason: string;
}
export interface AmountCorrection {
  previousTokens: Tokens;
  previousCostUsd: number | null;
  previousPartial: boolean;
  previousUnpricedReason: string | null;
  reason: string;
}
export interface RequestUsage {
  key: string; at: string; model: string | null;
  tokens: Tokens; rates: RateSnapshot | null; costUsd: number | null;
  partial: boolean; unpricedReason: string | null;
  serviceTier: string | null; tierSource: string | null;
  rateCorrection: RateCorrection | null;
  source: RequestSource | null;
  amountCorrection: AmountCorrection | null;
}
export interface TurnUsage {
  id: string; status: TurnStatus; requests: RequestUsage[];
  originThreadId: string | null;
  rootTurnId: string | null;
  parentTurnId: string | null;
}
export interface UsageSummary {
  input: number; cached: number; cacheWrite: number; output: number; reasoning: number; total: number;
  costUsd: number; unpricedRequests: number; requests: number; completedTurns: number;
  assumedTierRequests: number; missingCacheRequests: number;
  missingCachedRequests: number; missingCacheWriteRequests: number;
  unmeasuredTurns: number;
}

export interface Conversation {
  id: string; title: string; updatedAt: number; state: SessionState; available: boolean;
  usage: UsageSummary; // Existing meaning: this node's own usage.
  warning: string | null;
  lineage: Lineage;
  relationStatus: RelationStatus;
  rootSessionId: string | null;
  subagentCount: number; // Transitive accepted descendants; excludes this node.
  familyUsage: UsageSummary; // Selected node + accepted descendant subtree.
  coverage: Coverage;
  usageRevision: string; // Opaque cheap revision of own ingestion state; not persisted spending.
}
export interface Dashboard {
  sequence?: number;
  conversations: Conversation[]; // All nodes; UI hides resolved subagent rows.
  usage: UsageSummary; // Unique project spending, not sum(familyUsage).
  coverage: Coverage;
  pricingDate: string; warnings: string[];
}
export interface AgentUsageDetails {
  sessionId: string; title: string; available: boolean;
  lineage: Lineage; relationStatus: RelationStatus;
  ownUsage: UsageSummary; coverage: Coverage;
  turns: TurnUsage[]; // Owned turns only; copied foreign lifecycle is excluded.
}
export interface RequestRef { sessionId: string; turnId: string; requestKey: string }
export interface TurnRef { sessionId: string; turnId: string }
export interface PromptUsage {
  turnId: string; status: TurnStatus; // A real resolved-root own turn.
  ownUsage: UsageSummary; descendantsUsage: UsageSummary; combinedUsage: UsageSummary;
  requestRefs: RequestRef[];
  unmeasuredTurnRefs: TurnRef[];
}
export interface UnassignedUsage {
  usage: UsageSummary;
  requestRefs: RequestRef[];
  unmeasuredTurnRefs: TurnRef[];
}
export interface ConversationUsageDetails {
  sessionId: string; // Selected node, even if it is a child.
  rootSessionId: string | null; // Resolved ancestor information; does not widen selected scope.
  ownUsage: UsageSummary; descendantsUsage: UsageSummary; combinedUsage: UsageSummary;
  coverage: Coverage; warnings: string[];
  agents: AgentUsageDetails[]; // Selected node first, then its descendants; ancestors excluded.
  userPromptCount: number | null;
  agentTurnCount: number;
  prompts: PromptUsage[];
  unassigned: UnassignedUsage;
}
