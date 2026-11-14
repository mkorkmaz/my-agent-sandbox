import { createStore } from 'zustand/vanilla';

type RemovalPhase<T> = { phase: 'empty' } | { phase: 'reading'; revision: number }
  | { phase: 'reviewed' | 'removing'; value: T; reference: string };

export class RemovalReview<T> {
  private data = createStore<{ state: RemovalPhase<T> }>(() => ({ state: { phase: 'empty' } }));

  private get state(): RemovalPhase<T> { return this.data.getState().state; }
  private set state(state: RemovalPhase<T>) { this.data.setState({ state }); }

  current(): T | null { return 'value' in this.state ? this.state.value : null; }
  read(revision: number): void { this.state = { phase: 'reading', revision }; }
  review(revision: number, value: T, reference: string): boolean {
    if (this.state.phase !== 'reading' || this.state.revision !== revision) return false;
    this.state = { phase: 'reviewed', value, reference };
    return true;
  }
  reset(): void { this.state = { phase: 'empty' }; }
  confirms(confirmation: string): boolean { return this.state.phase === 'reviewed' && confirmation === this.state.reference; }

  remove(confirmation: string): T | null {
    if (this.state.phase !== 'reviewed' || !this.confirms(confirmation)) return null;
    this.state = { ...this.state, phase: 'removing' };
    return this.state.value;
  }

  retry(value: T): void {
    if (this.state.phase !== 'removing' || this.state.value !== value) return;
    this.state = { ...this.state, phase: 'reviewed' };
  }

  completed(value: T): void {
    if (this.state.phase !== 'removing' || this.state.value !== value) return;
    this.reset();
  }
}

