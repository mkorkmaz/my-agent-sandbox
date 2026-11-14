type LaunchState = 'idle' | 'checking' | 'starting' | 'published';

type CheckedEnvironment = { identifier: string; environmentId: string; fingerprint: string };
type AgentSession = CheckedEnvironment & { useBrigBoundary: boolean } & (
  { action: 'new-chat' | 'shell'; sessionId?: never } | { action: 'resume'; sessionId: string }
);
type ProjectOperation = 'git-status' | 'git-diff' | 'git-staged-diff' | 'tests' | 'doctor' | 'info' | 'policy-check' | 'logs';

export type TerminalLaunchRequest =
  | [command: 'terminal_start', arguments_: { projectId: string }, caption: string, foreground?: boolean]
  | [command: 'prepare_environment', arguments_: CheckedEnvironment, caption: string, foreground?: boolean]
  | [command: 'start_agent_session', arguments_: AgentSession, caption: string, foreground?: boolean]
  | [command: 'start_project_operation', arguments_: Omit<CheckedEnvironment, 'fingerprint'> & { fingerprint: string | null; operation: ProjectOperation }, caption: string, foreground?: boolean];
export type OpenTerminal = (...request: TerminalLaunchRequest) => Promise<void>;

export class TerminalLaunch {
  private state: LaunchState = 'idle';
  private closing = new Set<number>();

  begin(): boolean {
    if (this.blocksChanges()) return false;
    this.state = 'checking';
    return true;
  }

  start(): void { this.advance('checking', 'starting'); }
  publish(): void { this.advance('starting', 'published'); }
  finish(): void { this.state = 'idle'; }
  blocksChanges(): boolean { return this.state !== 'idle'; }

  close(identifier: number): boolean {
    if (this.closing.has(identifier)) return false;
    this.closing.add(identifier);
    return true;
  }

  closed(identifier: number): void { this.closing.delete(identifier); }
  isClosing(identifier: number): boolean { return this.closing.has(identifier); }

  private advance(expected: LaunchState, next: LaunchState): void {
    if (this.state !== expected) throw new Error('The terminal launch state changed.');
    this.state = next;
  }
}
