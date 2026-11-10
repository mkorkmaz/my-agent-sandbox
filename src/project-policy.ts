import { invoke } from '@tauri-apps/api/core';
import type { Project } from './projects';

interface Policy { document: unknown; fingerprint: string; editable: boolean; attached: boolean; allowedHosts: string[] }

export function initializeProjectPolicy(current: () => Project | null, changed: (project: Project, detached: boolean) => Promise<void>): void {
  const status = document.getElementById('project-policy-status')!;
  const fields = document.getElementById('project-policy-fields') as HTMLFieldSetElement;
  const hosts = document.getElementById('project-policy-hosts') as HTMLTextAreaElement;
  const save = document.getElementById('save-project-policy') as HTMLButtonElement;
  const attach = document.getElementById('attach-project-policy') as HTMLButtonElement;
  const detach = document.getElementById('detach-project-policy') as HTMLButtonElement;
  let snapshot: { projectId: string; policy: Policy } | null = null;
  let busy = false;
  document.getElementById('read-project-policy')!.addEventListener('click', async () => {
    const project = current(); if (!project || busy) return;
    busy = true; fields.disabled = true; snapshot = null;
    try {
      const policy = await invoke<Policy>('read_project_policy', { identifier: project.id });
      if (current()?.id !== project.id) return;
      snapshot = { projectId: project.id, policy };
      document.getElementById('project-policy-document')!.textContent = JSON.stringify(policy.document, null, 2);
      hosts.value = policy.allowedHosts.join('\n'); hosts.readOnly = !policy.editable;
      save.disabled = !policy.editable;
      attach.disabled = policy.attached;
      detach.disabled = !policy.attached || !project.ownsBinding;
      fields.disabled = false;
      status.textContent = policy.editable ? 'Stop the environment before editing allowed hosts. Other destinations are denied.' : 'This policy is externally managed, shared with other sessions or contains advanced rules. Editing is disabled.';
    } catch (error) { status.textContent = String(error); }
    finally { busy = false; }
  });
  for (const [id, action] of [['save-project-policy', 'save'], ['attach-project-policy', 'attach'], ['detach-project-policy', 'detach'], ['accept-project-policy', 'accept']]) {
    document.getElementById(id)!.addEventListener('click', async () => {
      if (!snapshot || snapshot.projectId !== current()?.id || busy) return;
      const captured = snapshot;
      busy = true; fields.disabled = true;
      try {
        const project = await invoke<Project>('change_project_policy', { identifier: captured.projectId, fingerprint: captured.policy.fingerprint, action, hosts: [...new Set(hosts.value.split('\n').map(host => host.trim()).filter(Boolean))] });
        snapshot = null;
        if (current()?.id !== project.id) return;
        await changed(project, action === 'detach');
        status.textContent = action === 'detach' ? 'Policy detached. Attach it again before starting the environment.' : 'Policy updated. Review the plan and prepare the environment again.';
      } catch (error) { status.textContent = String(error); fields.disabled = false; }
      finally { busy = false; }
    });
  }
}
