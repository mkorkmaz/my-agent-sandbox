import { invoke } from '@tauri-apps/api/core';
import { sameProjectView, type ProjectContext } from './projects';
import { setBusy } from './loading';

interface Probe { name: string; target: string; expected: string; observed: string; outcome: string }
interface Report { projectId: string; planFingerprint: string; checkedAt: number; passed: boolean; cleanup: boolean; results: Probe[] }

let verified: { project: ProjectContext; checkedAt: number; selected: boolean } | null = null;

function validFor(project: ProjectContext | null): boolean {
  const now = Math.floor(Date.now() / 1000);
  return !!verified && !!project && project.id === verified.project.id && project.epoch === verified.project.epoch
    && project.fingerprint !== null && project.fingerprint === verified.project.fingerprint
    && verified.checkedAt <= now && now - verified.checkedAt <= 3600;
}

export function useBrigBoundary(project: ProjectContext | null): boolean {
  return validFor(project) && verified!.selected;
}

export function resetBoundaryChoice(): void {
  verified = null;
  const choice = document.getElementById('use-brig-boundary') as HTMLInputElement;
  choice.checked = false;
  choice.disabled = true;
}

export function initializeBoundaryTests(current: () => ProjectContext | null): () => void {
  const run = document.getElementById('run-boundary-tests') as HTMLButtonElement;
  const status = document.getElementById('boundary-status')!;
  const results = document.getElementById('boundary-results')!;
  let request = 0;
  const choice = document.getElementById('use-brig-boundary') as HTMLInputElement;
  const renderChoice = () => {
    choice.disabled = !validFor(current());
    choice.checked = useBrigBoundary(current());
  };
  choice.addEventListener('change', () => {
    if (verified && validFor(current())) verified.selected = choice.checked;
    renderChoice();
  });
  run.addEventListener('click', async () => {
    const project = current();
    if (!project) return;
    const pending = ++request;
    run.disabled = true;
    setBusy(run, true);
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
      status.textContent = passed ? 'Brig file and network boundary checks passed. Check the inner agent sandbox in the same conversation.' : 'Some Brig boundaries could not be verified. Review the failed and inconclusive results.';
      if (!report.cleanup) status.textContent += ' Temporary files with an unverified identity were preserved.';
      if (passed && report.planFingerprint === project.fingerprint && report.planFingerprint === current()?.fingerprint) {
        verified = { project, checkedAt: report.checkedAt, selected: false };
      }
      renderChoice();
    } catch (error) { if (sameProjectView(current(), project) && pending === request) status.textContent = String(error); }
    finally { if (sameProjectView(current(), project) && pending === request) { setBusy(run, false); run.disabled = false; } }
  });
  const copy = document.getElementById('copy-inner-boundary-prompt') as HTMLButtonElement;
  copy.addEventListener('click', async () => {
    const context = current(); if (!context) return;
    copy.disabled = true; setBusy(copy, true);
    try {
      await invoke('copy_inner_boundary_prompt');
      if (sameProjectView(current(), context)) status.textContent = 'Check prompt copied. Paste it into the open conversation and review the results in the terminal.';
    } catch (error) { if (sameProjectView(current(), context)) status.textContent = String(error); }
    finally { if (sameProjectView(current(), context)) { copy.disabled = false; setBusy(copy, false); } }
  });
  return () => { ++request; run.disabled = false; copy.disabled = false; setBusy(run, false); setBusy(copy, false); status.textContent = ''; results.replaceChildren(); renderChoice(); };
}
