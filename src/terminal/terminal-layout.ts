type LayoutState = 'idle' | 'pending' | 'applying';

export class TerminalLayoutQueue {
  private state: LayoutState = 'idle';
  private work: Promise<void> | null = null;

  request(apply: () => Promise<void>, report: (error: unknown) => void): Promise<void> {
    this.state = 'pending';
    if (this.work) return this.work;
    this.work = this.drain(apply).catch(report).finally(() => {
      this.work = null;
      if (this.state === 'pending') return this.request(apply, report);
      this.state = 'idle';
    });
    return this.work;
  }

  private takePending(): boolean {
    if (this.state !== 'pending') return false;
    this.state = 'applying';
    return true;
  }

  private async drain(apply: () => Promise<void>): Promise<void> {
    while (this.takePending()) await apply();
  }
}
