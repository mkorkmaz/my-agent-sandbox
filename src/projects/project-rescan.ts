type RescanState = { phase: 'not-started' | 'idle' } | { phase: 'scanning'; revision: number }
  | { phase: 'waiting'; timer: number };

export class ProjectRescan {
  private state: RescanState = { phase: 'not-started' };
  private revision = 0;

  started(): boolean { return this.state.phase !== 'not-started'; }
  scanning(): boolean { return this.state.phase === 'scanning'; }

  begin(): { kind: 'initial' | 'repeat'; revision: number } | null {
    if (this.scanning()) return null;
    const kind = this.started() ? 'repeat' : 'initial';
    this.cancelTimer();
    const revision = ++this.revision;
    this.state = { phase: 'scanning', revision };
    return { kind, revision };
  }

  finish(revision: number): void {
    if (this.state.phase !== 'scanning' || this.state.revision !== revision) return;
    this.state = { phase: 'idle' };
  }

  schedule(needed: boolean, busy: boolean, run: () => Promise<void>): void {
    if (!needed || busy) { this.cancelTimer(); return; }
    if (this.state.phase !== 'idle') return;
    const timer = document.defaultView!.setTimeout(() => {
      if (this.state.phase !== 'waiting' || this.state.timer !== timer) return;
      this.state = { phase: 'idle' };
      void run();
    }, 8000);
    this.state = { phase: 'waiting', timer };
  }

  private cancelTimer(): void {
    if (this.state.phase !== 'waiting') return;
    document.defaultView!.clearTimeout(this.state.timer);
    this.state = { phase: 'idle' };
  }
}
