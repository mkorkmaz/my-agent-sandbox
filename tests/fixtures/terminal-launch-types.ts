import type { OpenTerminal } from '../../src/terminal/terminal-launch';

declare const open: OpenTerminal;
const environment = { identifier: 'project-a', environmentId: 'a'.repeat(32), fingerprint: 'reviewed-plan' };

void open('terminal_start', { projectId: 'project-a' }, 'Local terminal');
void open('prepare_environment', environment, 'Environment', false);
void open('start_agent_session', { ...environment, action: 'new-chat', useBrigBoundary: true }, 'Chat');
void open('start_agent_session', { ...environment, action: 'resume', sessionId: 'saved-session', useBrigBoundary: true }, 'Resume');
void open('start_project_operation', { ...environment, operation: 'git-diff' }, 'Diff');

// @ts-expect-error A resume must select its saved session.
void open('start_agent_session', { ...environment, action: 'resume', useBrigBoundary: true }, 'Resume');
// @ts-expect-error A new chat cannot resume a saved session.
void open('start_agent_session', { ...environment, action: 'new-chat', sessionId: 'saved-session', useBrigBoundary: true }, 'Chat');
// @ts-expect-error A local terminal uses projectId, not identifier.
void open('terminal_start', { identifier: 'project-a' }, 'Local terminal');
// @ts-expect-error Preparation requires the reviewed plan fingerprint.
void open('prepare_environment', { identifier: 'project-a', environmentId: 'environment' }, 'Environment');
// @ts-expect-error A project operation cannot carry an agent action.
void open('start_project_operation', { ...environment, action: 'shell' }, 'Operation');
