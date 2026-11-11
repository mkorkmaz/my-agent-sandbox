import { invoke } from '@tauri-apps/api/core';
import { sameProject, sameProjectView, type Project, type ProjectContext } from './projects';

interface Policy { document: unknown; fingerprint: string; editable: boolean; attached: boolean; allowedHosts: string[] }

export function initializeProjectPolicy(current: () => (ProjectContext & { project: Project }) | null, changed: (project: Project, detached: boolean) => Promise<void>): () => void {
  const status = document.getElementById('project-policy-status')!;
  const fields = document.getElementById('project-policy-fields') as HTMLFieldSetElement;
  const hosts = document.getElementById('project-policy-hosts') as HTMLTextAreaElement;
  const save = document.getElementById('save-project-policy') as HTMLButtonElement;
  const attach = document.getElementById('attach-project-policy') as HTMLButtonElement;
  const detach = document.getElementById('detach-project-policy') as HTMLButtonElement;
  const read = document.getElementById('read-project-policy') as HTMLButtonElement;
  let snapshot: { context: ProjectContext; policy: Policy } | null = null;
  let busy = false;
  let request = 0;
  read.addEventListener('click', async () => {
    const context = current(); if (!context || busy) return;
    const pending = ++request;
    busy = true; read.disabled = true; fields.disabled = true; snapshot = null;
    status.textContent = 'Reading this project’s policy…';
    document.getElementById('project-policy-document')!.textContent = '';
    hosts.value = '';
    try {
      const policy = await invoke<Policy>('read_project_policy', { identifier: context.id });
      if (!sameProjectView(current(), context) || pending !== request) return;
      snapshot = { context, policy };
      document.getElementById('project-policy-document')!.textContent = JSON.stringify(policy.document, null, 2);
      hosts.value = policy.allowedHosts.join('\n'); hosts.readOnly = !policy.editable;
      save.disabled = !policy.editable;
      attach.disabled = policy.attached;
      detach.disabled = !policy.attached || !context.project.ownsBinding;
      (document.getElementById('accept-project-policy') as HTMLButtonElement).disabled = false;
      fields.disabled = false;
      status.textContent = policy.editable ? 'Stop the environment before editing allowed guest hosts. Other guest command destinations are denied. Hosted Codex tools have a separate scope.' : 'This policy is externally managed, shared with other sessions or contains advanced rules. Editing is disabled.';
    } catch (error) { if (sameProjectView(current(), context) && pending === request) status.textContent = String(error); }
    finally { if (sameProjectView(current(), context) && pending === request) { busy = false; read.disabled = false; } }
  });
  for (const [id, action] of [['save-project-policy', 'save'], ['attach-project-policy', 'attach'], ['detach-project-policy', 'detach'], ['accept-project-policy', 'accept']]) {
    document.getElementById(id)!.addEventListener('click', async () => {
      if (!snapshot || !sameProjectView(current(), snapshot.context) || busy) return;
      const captured = snapshot;
      const pending = ++request;
      busy = true; read.disabled = true; fields.disabled = true;
      try {
        const project = await invoke<Project>('change_project_policy', { identifier: captured.context.id, fingerprint: captured.policy.fingerprint, action, hosts: [...new Set(hosts.value.split('\n').map(host => host.trim()).filter(Boolean))] });
        if (!sameProject(current(), captured.context)) return;
        if (sameProjectView(current(), captured.context) && pending === request) snapshot = null;
        const detached = project.policyDetached === true || project.stage === 'policy-detached';
        await changed(project, detached);
        if (!sameProjectView(current(), captured.context) || pending !== request) return;
        status.textContent = detached ? 'Policy detached. Attach it again before starting the environment.' : 'Policy updated. Review the plan and prepare the environment again.';
      } catch (error) { if (sameProjectView(current(), captured.context) && pending === request) { status.textContent = String(error); fields.disabled = false; } }
      finally { if (sameProjectView(current(), captured.context) && pending === request) { busy = false; read.disabled = false; } }
    });
  }
  return () => {
    ++request; busy = false; snapshot = null; read.disabled = false;
    fields.disabled = true; hosts.value = ''; hosts.readOnly = true; status.textContent = '';
    for (const button of fields.querySelectorAll<HTMLButtonElement>('button')) button.disabled = true;
    document.getElementById('project-policy-document')!.textContent = '';
  };
}
