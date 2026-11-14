type DragState = { phase: 'idle' } | { phase: 'dragging'; pointerId: number };
type SaveJob = { write: () => Promise<void>; failed: (error: unknown) => void };
type SaveState = { phase: 'idle' } | { phase: 'saving'; jobs: SaveJob[] };

export class SidebarDrag {
  private state: DragState = { phase: 'idle' };
  active(): boolean { return this.state.phase === 'dragging'; }
  start(pointerId: number): boolean {
    if (this.state.phase !== 'idle') return false;
    this.state = { phase: 'dragging', pointerId };
    return true;
  }
  moves(pointerId: number): boolean { return this.state.phase === 'dragging' && this.state.pointerId === pointerId; }
  end(pointerId: number): boolean {
    if (!this.moves(pointerId)) return false;
    this.state = { phase: 'idle' };
    return true;
  }
}

class PreferenceSaveQueue {
  private state: SaveState = { phase: 'idle' };

  add(job: SaveJob): void {
    if (this.state.phase === 'saving') { this.state.jobs.push(job); return; }
    this.state = { phase: 'saving', jobs: [job] };
    void Promise.resolve().then(() => this.drain());
  }

  private next(): SaveJob | undefined { return this.state.phase === 'saving' ? this.state.jobs.shift() : undefined; }

  private async drain(): Promise<void> {
    let job = this.next();
    while (job) { await this.write(job); job = this.next(); }
    this.state = { phase: 'idle' };
  }

  private async write(job: SaveJob): Promise<void> {
    try { await job.write(); }
    catch (error) { job.failed(error); }
  }
}

export class SidebarPreferences {
  private phase: 'loading' | 'restored' | 'edited' | 'failed' = 'loading';
  private saves = new PreferenceSaveQueue();

  restore(): boolean {
    if (this.phase === 'edited') return false;
    this.phase = 'restored';
    return true;
  }

  failed(): void { if (this.phase !== 'edited') this.phase = 'failed'; }
  edit(): void { this.phase = 'edited'; }
  migrate(job: SaveJob): void { this.saves.add(job); }
  save(job: SaveJob): void { this.edit(); this.saves.add(job); }
}
