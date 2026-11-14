type RequestPhase = { phase: 'idle' | 'pending' | 'settled'; revision: number };

export class RequestState {
  private state: RequestPhase = { phase: 'idle', revision: 0 };

  begin(): number {
    const revision = this.state.revision + 1;
    this.state = { phase: 'pending', revision };
    return revision;
  }

  invalidate(): void { this.state = { phase: 'idle', revision: this.state.revision + 1 }; }
  accepts(revision: number): boolean { return this.state.phase !== 'idle' && revision === this.state.revision; }
  pending(): boolean { return this.state.phase === 'pending'; }

  finish(revision: number): void {
    if (!this.pending() || !this.accepts(revision)) return;
    this.state = { phase: 'settled', revision };
  }
}
