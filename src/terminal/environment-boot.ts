import type { Project } from '../projects/projects';
import { startBootAnimation } from './boot-animation';
import type { TerminalRectangle } from './terminal-adapter';

type BootProject = Pick<Project, 'id' | 'environmentId'>;
type Attempt = { project: BootProject; startedAt: number };
type ConfirmedPhase = 'starting' | 'booting' | 'verifying';
type BootState = { phase: 'idle' } | ({ phase: 'starting'; verificationId?: number } & Attempt)
  | ({ phase: 'booting' | 'verifying' | 'cancelling' | 'awaiting-result'; identifier: number } & Attempt)
  | { phase: 'settled'; project: BootProject; identifier: number | null; confirmed: ConfirmedPhase; elapsed: string };

const messages = {
  starting: 'Waiting for Brig to start',
  booting: 'Brig started the guest setup process',
  verifying: 'Verifying the running guest and agent versions',
  cancelling: 'Stopping environment preparation',
  'awaiting-result': 'Waiting for the preparation result',
};

function elapsedTime(startedAt: number): string {
  const seconds = Math.floor((document.defaultView!.performance.now() - startedAt) / 1000);
  return Math.floor(seconds / 60) + ':' + String(seconds % 60).padStart(2, '0');
}

export class EnvironmentBoot {
  private state: BootState = { phase: 'idle' };
  private view = {
    overlay: document.getElementById('environment-boot-overlay')!,
    card: document.getElementById('environment-boot-card')!,
    message: document.getElementById('environment-boot-message')!,
    elapsed: document.getElementById('environment-boot-elapsed')!,
    canvas: document.getElementById('environment-boot-canvas') as HTMLCanvasElement,
    stop: null as (() => void) | null,
    timer: null as number | null,
    output: false,
  };

  begin(project: BootProject, identifier?: number): void {
    if (identifier !== undefined && this.hasIdentifier(project, identifier)) return;
    const startedAt = document.defaultView!.performance.now();
    this.view.output = false;
    this.state = identifier === undefined ? { phase: 'starting', project, startedAt } : { phase: 'booting', project, identifier, startedAt };
  }

  attach(project: BootProject, identifier: number): void {
    if (this.state.phase !== 'starting' || !this.matches(project)) return;
    this.state = { project, startedAt: this.state.startedAt, phase: this.state.verificationId === identifier ? 'verifying' : 'booting', identifier };
  }

  verify(project: BootProject, identifier: number): void {
    if (!this.matches(project) || this.state.phase === 'idle' || this.state.phase === 'settled') return;
    if (this.state.phase === 'starting') {
      this.state.verificationId = Math.max(this.state.verificationId ?? 0, identifier);
      return;
    }
    if (!this.hasIdentifier(project, identifier)) return;
    this.state = { ...this.state, phase: 'verifying' };
  }

  canCancel(project: BootProject | null, identifier: number | null): boolean {
    return this.state.phase === 'booting' && identifier === this.state.identifier && this.matches(project);
  }

  requestCancellation(project: BootProject, identifier: number): boolean {
    if (this.state.phase !== 'booting' || !this.canCancel(project, identifier)) return false;
    this.state = { ...this.state, phase: 'cancelling' };
    return true;
  }

  cancellationUnavailable(project: BootProject, identifier: number, retry: boolean): void {
    if (this.state.phase !== 'cancelling' || !this.hasIdentifier(project, identifier)) return;
    this.state = { ...this.state, phase: retry ? 'booting' : 'awaiting-result' };
  }

  finish(projectId: string, environmentId: string, identifier?: number): void {
    if (this.state.phase === 'idle' || this.state.phase === 'settled' || this.state.project.id !== projectId || this.state.project.environmentId !== environmentId) return;
    if ('identifier' in this.state && identifier !== undefined && this.state.identifier !== identifier) return;
    const confirmed = this.state.phase === 'starting' || this.state.phase === 'verifying' ? this.state.phase : 'booting';
    this.state = { phase: 'settled', project: this.state.project, identifier: identifier ?? ('identifier' in this.state ? this.state.identifier : null),
      confirmed, elapsed: elapsedTime(this.state.startedAt) };
    this.hide();
  }

  render(project: Project | null, identifier: number | null, visible: boolean): void {
    if (!visible || !this.matches(project) || this.state.phase === 'idle' || this.state.phase === 'settled') { this.hide(); return; }
    if ('identifier' in this.state && project?.stage !== 'environment-starting') {
      this.finish(this.state.project.id, this.state.project.environmentId, this.state.identifier);
      return;
    }
    if (this.view.output || ('identifier' in this.state && this.state.identifier !== identifier)) { this.hide(); return; }
    if (this.state.phase === 'starting' && identifier !== null) { this.hide(); return; }
    const message = messages[this.state.phase];
    if (this.view.message.textContent !== message) this.view.message.textContent = message;
    this.view.overlay.hidden = false;
    this.renderElapsed();
    this.view.timer ??= document.defaultView!.setInterval(() => { this.renderElapsed(); }, 1000);
    if (!this.view.stop) this.view.stop = startBootAnimation(this.view.canvas);
  }

  bounds(surface: DOMRect): TerminalRectangle | undefined {
    if (this.view.overlay.hidden) return;
    const card = this.view.card.getBoundingClientRect();
    const left = Math.max(surface.left, card.left), top = Math.max(surface.top, card.top);
    const right = Math.min(surface.right, card.right), bottom = Math.min(surface.bottom, card.bottom);
    return { x: left - surface.left, y: top - surface.top, width: Math.max(0, right - left), height: Math.max(0, bottom - top) };
  }

  blocksFocus(identifier: number): boolean {
    if (this.view.overlay.hidden || this.state.phase === 'idle' || this.state.phase === 'settled') return false;
    return this.state.phase === 'starting' || this.state.identifier === identifier;
  }

  pending(project: BootProject | null): boolean {
    return this.matches(project) && this.state.phase !== 'idle' && this.state.phase !== 'settled';
  }

  hasOutput(project: BootProject | null, identifier: number | null): boolean {
    return identifier !== null && this.pending(project) && this.hasIdentifier(project, identifier);
  }

  toggleOutput(): void { this.view.output = !this.view.output; }
  showingOutput(): boolean { return this.view.output; }

  lastStage(project: BootProject | null): string | null {
    if (this.state.phase !== 'settled' || !this.matches(project)) return null;
    return 'Last confirmed stage — ' + messages[this.state.confirmed] + ' (' + this.state.elapsed + ' elapsed)';
  }

  private hasIdentifier(project: BootProject | null, identifier: number): boolean {
    return 'identifier' in this.state && this.state.identifier === identifier && this.matches(project);
  }

  private renderElapsed(): void {
    if ('startedAt' in this.state) this.view.elapsed.textContent = 'Elapsed ' + elapsedTime(this.state.startedAt);
  }

  private matches(project: BootProject | null): boolean {
    return this.state.phase !== 'idle' && project !== null && this.state.project.id === project.id && this.state.project.environmentId === project.environmentId;
  }

  private hide(): void {
    this.view.overlay.hidden = true;
    this.view.stop?.();
    this.view.stop = null;
    if (this.view.timer !== null) document.defaultView!.clearInterval(this.view.timer);
    this.view.timer = null;
  }
}
