type StartupStep = 'window' | 'controllers' | 'records' | 'settings' | 'chatgpt' | 'claude' | 'projects';
type StartupState = { phase: 'waiting' | 'running' | 'failed'; step: StartupStep } | { phase: 'ready' };
const nextStep: Record<StartupStep, StartupStep | null> = {
  window: 'controllers', controllers: 'records', records: 'settings', settings: 'chatgpt',
  chatgpt: 'claude', claude: 'projects', projects: null,
};

export class StartupSequence {
  private state: StartupState = { phase: 'waiting', step: 'window' };

  async run<T>(step: StartupStep, action: () => T | Promise<T>): Promise<T> {
    if (this.state.phase === 'ready' || this.state.phase === 'running' || this.state.step !== step) {
      throw new Error('The app startup step is out of order.');
    }
    this.state = { phase: 'running', step };
    try {
      const result = await action();
      const next = nextStep[step];
      this.state = next ? { phase: 'waiting', step: next } : { phase: 'ready' };
      return result;
    } catch (error) {
      this.state = { phase: 'failed', step };
      throw error;
    }
  }
}
