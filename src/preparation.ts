import { invoke } from '@tauri-apps/api/core';
import type { Project } from './projects';

export interface Plan {
  data: {
    profile: string;
    session: string;
    runtime: { kind: string; backend: string };
    isolation: string;
    project: { host: string; guest: string; mode: string };
    home: { host: string; guest: string };
    limits: { cpus: number; mem: number };
    posture: string;
    image: { ref: string };
    verify: { mode: string };
    policies: string[];
    egress: { default: string; allow?: { host?: string; cidr?: string }[] };
    credentials: { name: string; state: string }[];
  };
}
interface Preparation { project: Project; plan: Plan | null; candidates: string[]; message: string | null; fingerprint: string | null }

export async function prepareProject(project: Project, chosenReference: string | null = null): Promise<Preparation> {
  return invoke<Preparation>('prepare_project', { identifier: project.id, chosenReference });
}

export function renderPlan(plan: Plan): void {
  const container = document.getElementById('sharing-plan')!;
  const list = document.getElementById('sharing-plan-values')!;
  const data = plan.data;
  const values: [string, string][] = [
    ['Brig profile and session', data.profile + '@' + data.session],
    ['Runtime', data.runtime.kind + ' / ' + data.runtime.backend], ['Isolation', data.isolation],
    ['Writable folder', data.project.host], ['Guest project path', data.project.guest],
    ['Guest home', data.home.host], ['Resources', data.limits.cpus + ' CPU · ' + data.limits.mem + ' MB'],
    ['Guest network', data.posture === 'offline' ? 'Offline for guest commands' : 'Guest command traffic allowed by Brig policy'],
    ['Image', data.image.ref], ['Signature verification', data.verify.mode],
    ['Policies', data.policies.join(', ')],
    ['Allowed guest destinations', data.egress.allow?.map(rule => rule.host ?? rule.cidr ?? '').join(', ') || 'None'],
    ['Hosted agent tools', 'Separate from the guest network policy'],
    ['Agent web search', 'Mode not read or managed by Tugboat'],
    ['Host credentials', data.credentials.map(credential => credential.name + ' (' + (credential.state === 'unresolved' ? 'unresolved' : 'withheld') + ')').join(', ') || 'None'],
  ];
  list.replaceChildren();
  for (const [label, value] of values) {
    const row = document.createElement('div');
    const title = document.createElement('dt'); title.textContent = label;
    const description = document.createElement('dd'); description.textContent = value;
    row.append(title, description); list.append(row);
  }
  container.hidden = false;
}
