import type { ConversationUsageDetails } from './usage-types';
import { createStore } from 'zustand/vanilla';

type UsageSelection = { projectId: string; sessionId: string; environmentId: string | null; fingerprint: string; revision: number };
type UsageRequest = UsageSelection & { attempt: number };
type UsageLoad = { phase: 'empty' | 'failed-empty' }
  | { phase: 'loading'; request: UsageRequest }
  | { phase: 'refreshing'; details: ConversationUsageDetails; request: UsageRequest }
  | { phase: 'ready' | 'failed-retained'; details: ConversationUsageDetails };
type UsageView = { phase: 'closed' } | { phase: 'open'; selection: UsageSelection; load: UsageLoad };

export class UsageDetailsState {
  private data = createStore<{ state: UsageView; cached: Map<string, { fingerprint: string; details: ConversationUsageDetails }> }>(() => ({ state: { phase: 'closed' }, cached: new Map() }));
  private ordering = { view: 0, attempt: 0 };

  private get state(): UsageView { return this.data.getState().state; }
  private set state(state: UsageView) { this.data.setState({ state }); }

  open(projectId: string, sessionId: string, environmentId: string | null, fingerprint: string): number {
    const revision = ++this.ordering.view;
    const selection = { projectId, sessionId, environmentId, fingerprint, revision };
    const cached = this.data.getState().cached.get(this.key(selection));
    const load: UsageLoad = cached?.fingerprint === fingerprint ? { phase: 'ready', details: cached.details } : { phase: 'empty' };
    this.state = { phase: 'open', selection, load };
    return revision;
  }

  close(): void { ++this.ordering.view; this.state = { phase: 'closed' }; }
  current(): Readonly<UsageSelection> | null { return this.state.phase === 'open' ? this.state.selection : null; }
  accepts(sessionId: string, revision: number): boolean {
    return this.state.phase === 'open' && this.state.selection.sessionId === sessionId && this.ordering.view === revision;
  }
  unchanged(fingerprint: string): boolean { return this.state.phase === 'open' && this.state.selection.fingerprint === fingerprint; }
  details(): ConversationUsageDetails | null {
    return this.state.phase === 'open' && 'details' in this.state.load ? this.state.load.details : null;
  }

  refresh(fingerprint: string): boolean {
    const state = this.state;
    if (state.phase === 'closed' || this.unchanged(fingerprint)) return false;
    this.state = { ...state, selection: { ...state.selection, fingerprint } };
    return true;
  }

  begin(): (UsageRequest & { details: ConversationUsageDetails | null }) | null {
    const state = this.state;
    if (state.phase === 'closed' || 'request' in state.load) return null;
    const details = this.details();
    const request = { ...state.selection, attempt: ++this.ordering.attempt };
    this.state = { ...state, load: details ? { phase: 'refreshing', details, request } : { phase: 'loading', request } };
    return { ...request, details };
  }

  show(request: UsageRequest, details: ConversationUsageDetails): boolean {
    if (!this.active(request) || !this.unchanged(request.fingerprint) || this.state.phase === 'closed') return false;
    if (details.sessionId !== request.sessionId || !details.agents.some(agent => agent.sessionId === request.sessionId)) {
      throw new Error('The recorded usage response does not match this session. Refresh the session list and try again.');
    }
    const cached = new Map(this.data.getState().cached);
    cached.set(this.key(request), { fingerprint: request.fingerprint, details });
    this.data.setState({ cached });
    this.state = { ...this.state, load: { phase: 'ready', details } };
    return true;
  }

  fail(request: UsageRequest): boolean {
    if (!this.active(request) || !this.unchanged(request.fingerprint) || this.state.phase === 'closed') return false;
    const details = this.details();
    this.state = { ...this.state, load: details ? { phase: 'failed-retained', details } : { phase: 'failed-empty' } };
    return true;
  }

  finish(request: UsageRequest): void {
    if (!this.active(request) || this.state.phase === 'closed') return;
    const details = this.details();
    this.state = { ...this.state, load: details ? { phase: 'ready', details } : { phase: 'empty' } };
  }

  private active(request: UsageRequest): boolean {
    if (this.state.phase === 'closed' || !('request' in this.state.load)) return false;
    return this.accepts(request.sessionId, request.revision)
      && this.state.selection.environmentId === request.environmentId
      && this.state.load.request.attempt === request.attempt;
  }

  clearCache(): void { this.data.setState({ cached: new Map() }); }

  private key(selection: Pick<UsageSelection, 'projectId' | 'environmentId' | 'sessionId'>): string {
    return JSON.stringify([selection.projectId, selection.environmentId, selection.sessionId]);
  }
}
