import type { UsageSummary } from './usage-types';

export const emptyUsage: UsageSummary = { input: 0, cached: 0, cacheWrite: 0, output: 0, reasoning: 0, total: 0, costUsd: 0, unpricedRequests: 0, requests: 0, completedTurns: 0, assumedTierRequests: 0, missingCacheRequests: 0, missingCachedRequests: 0, missingCacheWriteRequests: 0, unmeasuredTurns: 0 };
export function count(value: number): string { return new Intl.NumberFormat('en-US').format(value); }
function compactCount(value: number): string { return value >= 10000 ? new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 }).format(value) : count(value); }
export function cachePercentage(cached: number, input: number): string {
  return new Intl.NumberFormat('en-US', { style: 'percent', maximumFractionDigits: 1 }).format(input > 0 ? cached / input : 0);
}
export function usageCount(usage: UsageSummary, field: 'input' | 'output' | 'total', compact = false): string {
  const value = compact ? compactCount(usage[field]) : count(usage[field]);
  if (!usage.unmeasuredTurns) return value;
  return usage.requests ? value + ' + unknown' : 'Unknown';
}
export function cachedCount(usage: UsageSummary, compact = false): string {
  const value = compact ? compactCount(usage.cached) : count(usage.cached);
  if (usage.missingCachedRequests === usage.requests && usage.requests > 0 || usage.unmeasuredTurns && !usage.requests) return 'Unknown';
  if (usage.missingCachedRequests || usage.unmeasuredTurns) return value + ' + unknown';
  return value + ' (' + cachePercentage(usage.cached, usage.input) + ')';
}
export function cacheWrites(usage: UsageSummary): string {
  if (usage.missingCacheWriteRequests === usage.requests && usage.requests > 0 || usage.unmeasuredTurns && !usage.requests) return 'Unknown';
  if (usage.missingCacheWriteRequests || usage.unmeasuredTurns) return count(usage.cacheWrite) + ' + unknown';
  return count(usage.cacheWrite);
}
export function money(usage: UsageSummary): string {
  if (usage.unmeasuredTurns && !usage.requests) return 'Unknown';
  if (usage.unpricedRequests && usage.requests === usage.unpricedRequests) return 'Unavailable';
  const amount = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: 2, maximumFractionDigits: 4 }).format(usage.costUsd);
  return amount + (usage.unpricedRequests ? ' + unpriced' : '') + (usage.unmeasuredTurns ? ' + unknown' : '');
}
export function summaryText(usage: UsageSummary): string {
  return usageCount(usage, 'total') + ' tokens · ' + usageCount(usage, 'input') + ' input · ' + cachedCount(usage) + ' cached input · ' + cacheWrites(usage) + ' cache writes · ' + usageCount(usage, 'output') + ' output · ' + money(usage) + ' API equivalent';
}
