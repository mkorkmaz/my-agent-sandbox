import { RequestState } from './request-state';
import { createStore } from 'zustand/vanilla';

type FormPhase<T> = { phase: 'empty' | 'reading' | 'failed-empty' }
  | { phase: 'ready' | 'saving' | 'saved' | 'failed-retained'; value: T };

export class FormState<T> {
  private data = createStore<{ state: FormPhase<T> }>(() => ({ state: { phase: 'empty' } }));
  private requests = new RequestState();

  private get state(): FormPhase<T> { return this.data.getState().state; }
  private set state(state: FormPhase<T>) { this.data.setState({ state }); }

  current(): T | null { return 'value' in this.state ? this.state.value : null; }
  busy(): boolean { return this.requests.pending(); }
  accepts(revision: number): boolean { return this.requests.accepts(revision); }
  receive(revision: number, value: T): void {
    if (!this.active(revision) || !['reading', 'saving'].includes(this.state.phase)) return;
    this.state = { phase: 'ready', value };
  }
  saved(revision: number, value: T): void {
    if (!this.active(revision) || this.state.phase !== 'saving') return;
    this.state = { phase: 'saved', value };
  }

  replace(value: T): boolean {
    if (this.busy()) return false;
    this.state = { phase: 'ready', value };
    return true;
  }

  read(): number {
    this.state = { phase: 'reading' };
    return this.requests.begin();
  }

  submit(): { revision: number; value: T } | null {
    const value = this.current();
    if (value === null || this.busy()) return null;
    this.state = { phase: 'saving', value };
    return { revision: this.requests.begin(), value };
  }

  fail(revision: number): void {
    if (!this.active(revision)) return;
    const value = this.current();
    this.state = value === null ? { phase: 'failed-empty' } : { phase: 'failed-retained', value };
  }

  finish(revision: number): void {
    if (!this.accepts(revision)) return;
    this.requests.finish(revision);
    if (this.state.phase === 'reading') this.state = { phase: 'empty' };
    if (this.state.phase === 'saving') this.state = { phase: 'ready', value: this.state.value };
  }

  discard(revision: number): void {
    if (!this.active(revision)) return;
    this.state = { phase: 'empty' };
  }
  reset(): void { this.requests.invalidate(); this.state = { phase: 'empty' }; }

  private active(revision: number): boolean { return this.busy() && this.accepts(revision); }
}
