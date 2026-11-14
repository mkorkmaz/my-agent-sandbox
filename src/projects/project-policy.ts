import { invoke } from '@tauri-apps/api/core';
import { sameProject, sameProjectView, type Project, type ProjectContext } from './projects';
import { renderSkeleton, setBusy, setLoading } from '../ui/loading';
import { FormState } from '../ui/form-state';

interface Policy { document: unknown; fingerprint: string; editable: boolean; attached: boolean; allowedHosts: string[] }

export function initializeProjectPolicy(current: () => (ProjectContext & { project: Project }) | null, changed: (project: Project, detached: boolean) => Promise<void>): () => void {
  const status = document.getElementById('project-policy-status')!;
  const fields = document.getElementById('project-policy-fields') as HTMLFieldSetElement;
  const hosts = document.getElementById('project-policy-hosts') as HTMLTextAreaElement;
  const save = document.getElementById('save-project-policy') as HTMLButtonElement;
  const attach = document.getElementById('attach-project-policy') as HTMLButtonElement;
  const detach = document.getElementById('detach-project-policy') as HTMLButtonElement;
  const read = document.getElementById('read-project-policy') as HTMLButtonElement;
  const documentView = document.getElementById('project-policy-document')!;
  const state = new FormState<{ context: ProjectContext; policy: Policy }>();
  read.addEventListener('click', async () => {
    const context = current(); if (!context || state.busy()) return;
    const pending = state.read();
    read.disabled = true; fields.disabled = true;
    setBusy(read, true); setLoading(documentView, true);
    status.textContent = 'Reading this project’s policy…';
    renderSkeleton(documentView);
    hosts.value = '';
    try {
      const policy = await invoke<Policy>('read_project_policy', { identifier: context.id, environmentId: context.environmentId });
      if (!sameProjectView(current(), context) || !state.accepts(pending)) return;
      state.receive(pending, { context, policy });
      const saved = state.current()!.policy;
      documentView.textContent = JSON.stringify(saved.document, null, 2);
      hosts.value = saved.allowedHosts.join('\n'); hosts.readOnly = !saved.editable;
      save.disabled = !saved.editable;
      attach.disabled = saved.attached;
      detach.disabled = !saved.attached || !context.project.ownsBinding;
      (document.getElementById('accept-project-policy') as HTMLButtonElement).disabled = false;
      fields.disabled = false;
      status.textContent = saved.editable ? 'Stop the environment before editing allowed guest hosts. Other guest command destinations are denied. Hosted agent tools have a separate scope.' : 'This policy is externally managed, shared with other sessions or contains advanced rules. Editing is disabled.';
    } catch (error) { if (sameProjectView(current(), context) && state.accepts(pending)) { state.fail(pending); status.textContent = String(error); } }
    finally { if (sameProjectView(current(), context) && state.accepts(pending)) { state.finish(pending); read.disabled = false; setBusy(read, false); setLoading(documentView, false); if (!state.current()) documentView.replaceChildren(); } }
  });
  for (const [id, action] of [['save-project-policy', 'save'], ['attach-project-policy', 'attach'], ['detach-project-policy', 'detach'], ['accept-project-policy', 'accept']]) {
    document.getElementById(id)!.addEventListener('click', async () => {
      const snapshot = state.current();
      if (!snapshot || !sameProjectView(current(), snapshot.context)) return;
      const submission = state.submit();
      if (!submission) return;
      const captured = submission.value;
      const pending = submission.revision;
      read.disabled = true; fields.disabled = true;
      const button = document.getElementById(id)!; setBusy(button, true);
      status.textContent = 'Updating this project’s policy…';
      try {
        const project = await invoke<Project>('change_project_policy', { identifier: captured.context.id, environmentId: captured.context.environmentId, fingerprint: captured.policy.fingerprint, action, hosts: [...new Set(hosts.value.split('\n').map(host => host.trim()).filter(Boolean))] });
        if (!sameProject(current(), captured.context)) return;
        if (sameProjectView(current(), captured.context) && state.accepts(pending)) state.discard(pending);
        const detached = project.policyDetached === true || project.stage === 'policy-detached';
        await changed(project, detached);
        if (!sameProjectView(current(), captured.context) || !state.accepts(pending)) return;
        status.textContent = detached ? 'Policy detached. Attach it again before starting the environment.' : 'Policy updated. Review the plan and prepare the environment again.';
      } catch (error) { if (sameProjectView(current(), captured.context) && state.accepts(pending)) { state.fail(pending); status.textContent = String(error); fields.disabled = false; } }
      finally { if (sameProjectView(current(), captured.context) && state.accepts(pending)) { state.finish(pending); read.disabled = false; setBusy(button, false); } }
    });
  }
  return () => {
    state.reset(); read.disabled = false;
    setBusy(read, false); setLoading(documentView, false);
    fields.disabled = true; hosts.value = ''; hosts.readOnly = true; status.textContent = '';
    for (const button of fields.querySelectorAll<HTMLButtonElement>('button')) { button.disabled = true; setBusy(button, false); }
    documentView.replaceChildren();
  };
}
