import { RequestState } from '../ui/request-state';
import { createStore } from 'zustand/vanilla';
import type { Dashboard } from './dashboard';

type DashboardView = { phase: 'empty' } | { phase: 'ready'; value: Dashboard }
  | { phase: 'failed-empty'; message: string }
  | { phase: 'failed-retained'; value: Dashboard; message: string };
export type DashboardRequest = { projectId: string; revision: number; updates: number };

export class DashboardState {
  private data = createStore<{ view: DashboardView; cached: Map<string, Dashboard> }>(() => ({ view: { phase: 'empty' }, cached: new Map() }));
  private ordering = { requests: new RequestState(), updates: 0, sequence: 0, projectId: null as string | null };

  private get view(): DashboardView { return this.data.getState().view; }
  private set view(view: DashboardView) { this.data.setState({ view }); }

  current(): Dashboard | null { return 'value' in this.view ? this.view.value : null; }
  loaded(): boolean { return this.current() !== null; }
  matches(projectId: string): boolean { return this.ordering.projectId === projectId; }
  error(): string | null { return 'message' in this.view ? this.view.message : null; }
  pending(): boolean { return this.ordering.requests.pending(); }
  accepts(request: DashboardRequest): boolean {
    return request.projectId === this.ordering.projectId && this.ordering.requests.accepts(request.revision);
  }
  finish(request: DashboardRequest): void {
    if (this.accepts(request)) this.ordering.requests.finish(request.revision);
  }

  begin(): DashboardRequest | null {
    const projectId = this.ordering.projectId;
    if (projectId === null) return null;
    const value = this.current();
    this.view = value ? { phase: 'ready', value } : { phase: 'empty' };
    return { projectId, revision: this.ordering.requests.begin(), updates: this.ordering.updates };
  }

  reset(projectId: string | null): void {
    this.ordering.requests.invalidate();
    this.ordering.projectId = projectId;
    const value = projectId === null ? undefined : this.data.getState().cached.get(projectId);
    this.ordering.sequence = value?.sequence ?? 0;
    this.view = value ? { phase: 'ready', value } : { phase: 'empty' };
  }

  snapshot(request: DashboardRequest, value: Dashboard): boolean {
    if (!this.pending() || !this.accepts(request)) return false;
    if (value.sequence === undefined && request.updates !== this.ordering.updates) return false;
    if (!this.acceptUpdate(value.sequence)) return false;
    this.view = { phase: 'ready', value };
    this.remember(request.projectId, value);
    this.finish(request);
    return true;
  }

  event(projectId: string, value: Dashboard): boolean {
    if (projectId !== this.ordering.projectId) { this.remember(projectId, value); return false; }
    if (!this.acceptUpdate(value.sequence)) return false;
    this.view = { phase: 'ready', value };
    this.remember(projectId, value);
    return true;
  }

  forget(projectId: string): void {
    const cached = new Map(this.data.getState().cached);
    cached.delete(projectId);
    this.data.setState({ cached });
  }

  projectIds(): string[] { return [...this.data.getState().cached.keys()]; }
  cachedDashboard(projectId: string): Dashboard | null { return this.data.getState().cached.get(projectId) ?? null; }

  fail(request: DashboardRequest, message: string): boolean {
    if (!this.pending() || !this.accepts(request) || request.updates !== this.ordering.updates) return false;
    this.failView(message);
    this.finish(request);
    return true;
  }

  errorEvent(projectId: string, sequence: number | undefined, message: string): boolean {
    if (projectId !== this.ordering.projectId || !this.acceptUpdate(sequence)) return false;
    this.failView(message);
    return true;
  }

  private failView(message: string): void {
    const value = this.current();
    this.view = value ? { phase: 'failed-retained', value, message } : { phase: 'failed-empty', message };
  }

  private acceptUpdate(sequence?: number): boolean {
    if (sequence !== undefined && sequence <= this.ordering.sequence) return false;
    if (sequence !== undefined) this.ordering.sequence = sequence;
    this.ordering.updates++;
    return true;
  }

  private remember(projectId: string, value: Dashboard): void {
    const cached = new Map(this.data.getState().cached);
    const previous = cached.get(projectId);
    if (previous?.sequence !== undefined && (value.sequence === undefined || value.sequence <= previous.sequence)) return;
    cached.set(projectId, value);
    this.data.setState({ cached });
  }
}
