import { agentName, type Project } from '../projects/projects';
import { startBootAnimation } from './boot-animation';
import type { TerminalRectangle } from './terminal-adapter';

type BootProject = Pick<Project, 'id' | 'environmentId' | 'agentKind'>;
type BootState = { phase: 'idle' } | { phase: 'starting'; project: BootProject }
  | { phase: 'booting'; project: BootProject; identifier: number }
  | { phase: 'settled'; project: BootProject; identifier: number | null };

export class EnvironmentBoot {
  private state: BootState = { phase: 'idle' };
  private view = {
    overlay: document.getElementById('environment-boot-overlay')!,
    card: document.getElementById('environment-boot-card')!,
    message: document.getElementById('environment-boot-message')!,
    canvas: document.getElementById('environment-boot-canvas') as HTMLCanvasElement,
    stop: null as (() => void) | null,
  };

  begin(project: BootProject, identifier?: number): void {
    if (identifier !== undefined && this.state.phase === 'settled' && this.matches(project) && this.state.identifier === identifier) return;
    this.state = identifier === undefined ? { phase: 'starting', project } : { phase: 'booting', project, identifier };
  }

  attach(project: BootProject, identifier: number): void {
    if (this.state.phase !== 'starting' || !this.matches(project)) return;
    this.state = { phase: 'booting', project, identifier };
  }

  finish(projectId: string, environmentId: string, identifier?: number): void {
    if (this.state.phase === 'idle' || this.state.phase === 'settled' || this.state.project.id !== projectId || this.state.project.environmentId !== environmentId) return;
    if (this.state.phase === 'booting' && identifier !== undefined && this.state.identifier !== identifier) return;
    this.state = { phase: 'settled', project: this.state.project, identifier: identifier ?? (this.state.phase === 'booting' ? this.state.identifier : null) };
    this.hide();
  }

  render(project: Project | null, identifier: number | null, visible: boolean): void {
    if (!visible || !this.matches(project) || this.state.phase === 'idle' || this.state.phase === 'settled') { this.hide(); return; }
    if (this.state.phase === 'booting' && project?.stage !== 'environment-starting') {
      this.finish(this.state.project.id, this.state.project.environmentId, this.state.identifier);
      return;
    }
    if (this.state.phase === 'booting' && this.state.identifier !== identifier) { this.hide(); return; }
    if (this.state.phase === 'starting' && identifier !== null) { this.hide(); return; }
    const message = agentName(this.state.project.agentKind) + ' environment booting';
    if (this.view.message.textContent !== message) this.view.message.textContent = message;
    this.view.overlay.hidden = false;
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
    return this.matches(project) && (this.state.phase === 'starting' || this.state.phase === 'booting');
  }

  private matches(project: BootProject | null): boolean {
    return this.state.phase !== 'idle' && project !== null && this.state.project.id === project.id && this.state.project.environmentId === project.environmentId;
  }

  private hide(): void {
    this.view.overlay.hidden = true;
    this.view.stop?.();
    this.view.stop = null;
  }
}
