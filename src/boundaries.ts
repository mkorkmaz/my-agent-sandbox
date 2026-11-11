import { invoke } from '@tauri-apps/api/core';
import { sameProjectView, type ProjectContext } from './projects';

interface Probe { name: string; target: string; expected: string; observed: string; outcome: string }
interface Report { projectId: string; planFingerprint: string; passed: boolean; cleanup: boolean; results: Probe[] }

export function resetBoundaryChoice(): void {
  const choice = document.getElementById('use-brig-boundary') as HTMLInputElement;
  choice.checked = false;
  choice.disabled = true;
}

export function initializeBoundaryTests(current: () => ProjectContext | null): () => void {
  const run = document.getElementById('run-boundary-tests') as HTMLButtonElement;
  const status = document.getElementById('boundary-status')!;
  const results = document.getElementById('boundary-results')!;
  let request = 0;
  run.addEventListener('click', async () => {
    const project = current();
    if (!project) return;
    const pending = ++request;
    run.disabled = true;
    status.textContent = 'Checking file sharing and network boundaries…';
    resetBoundaryChoice();
    try {
      const report = await invoke<Report>('run_boundary_tests', { identifier: project.id });
      if (!sameProjectView(current(), project) || pending !== request || report.projectId !== project.id) return;
      results.replaceChildren();
      const labels: Record<string, string> = { pass: 'Passed', fail: 'Failed', inconclusive: 'Inconclusive' };
      for (const probe of report.results) {
        const row = document.createElement('li');
        const heading = document.createElement('strong'); heading.textContent = probe.name + ' · ' + labels[probe.outcome];
        const target = document.createElement('p'); target.textContent = probe.target; target.className = 'probe-target';
        const expected = document.createElement('p'); expected.textContent = 'Expected: ' + probe.expected;
        const observed = document.createElement('p'); observed.textContent = 'Observed: ' + probe.observed;
        row.append(heading, target, expected, observed); results.append(row);
      }
      const passed = report.passed && report.cleanup && report.results.length > 0 && report.results.every(probe => probe.outcome === 'pass');
      status.textContent = passed ? 'Brig file and network boundary checks passed. Check the inner Codex sandbox in the same conversation.' : 'Some Brig boundaries could not be verified. Review the failed and inconclusive results.';
      if (!report.cleanup) status.textContent += ' Temporary files with an unverified identity were preserved.';
      const choice = document.getElementById('use-brig-boundary') as HTMLInputElement;
      choice.disabled = !(passed && report.planFingerprint === current()?.fingerprint);
    } catch (error) { if (sameProjectView(current(), project) && pending === request) status.textContent = String(error); }
    finally { if (sameProjectView(current(), project) && pending === request) run.disabled = false; }
  });
  document.getElementById('copy-inner-boundary-prompt')!.addEventListener('click', async () => {
    const context = current(); if (!context) return;
    try {
      await invoke('copy_inner_boundary_prompt');
      if (sameProjectView(current(), context)) status.textContent = 'Check prompt copied. Paste it into the open Codex conversation and review the results in the terminal.';
    } catch (error) { if (sameProjectView(current(), context)) status.textContent = String(error); }
  });
  return () => { ++request; run.disabled = false; status.textContent = ''; results.replaceChildren(); resetBoundaryChoice(); };
}
