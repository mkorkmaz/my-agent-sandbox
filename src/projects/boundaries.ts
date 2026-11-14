import { invoke } from '@tauri-apps/api/core';
import { sameProjectView, type ProjectContext } from './projects';
import { setBusy } from '../ui/loading';
import { RequestState } from '../ui/request-state';
import { createStore } from 'zustand/vanilla';

interface Probe { name: string; target: string; expected: string; observed: string; outcome: string }
interface Report { projectId: string; environmentId: string; planFingerprint: string; checkedAt: number; passed: boolean; cleanup: boolean; results: Probe[] }

export function useBrigBoundary(): boolean {
  return (document.getElementById('use-brig-boundary') as HTMLInputElement).checked;
}

export function resetBoundaryChoice(): void {
  const choice = document.getElementById('use-brig-boundary') as HTMLInputElement;
  choice.checked = true;
}

export function initializeBoundaryTests(current: () => ProjectContext | null): () => void {
  const run = document.getElementById('run-boundary-tests') as HTMLButtonElement;
  const status = document.getElementById('boundary-status')!;
  const results = document.getElementById('boundary-results')!;
  const checks = new RequestState();
  const copying = new RequestState();
  const data = createStore<{ report: Report | null }>(() => ({ report: null }));
  run.addEventListener('click', async () => {
    const project = current();
    if (!project) return;
    const pending = checks.begin();
    run.disabled = true;
    setBusy(run, true);
    status.textContent = 'Checking file sharing and network boundaries…';
    try {
      const received = await invoke<Report>('run_boundary_tests', { identifier: project.id, environmentId: project.environmentId });
      if (!sameProjectView(current(), project) || current()?.fingerprint !== project.fingerprint || !checks.accepts(pending)) return;
      if (received.projectId !== project.id || received.environmentId !== project.environmentId || received.planFingerprint !== project.fingerprint) throw new Error('The boundary report belongs to another environment or plan. Run the check again.');
      data.setState({ report: received });
      const report = data.getState().report!;
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
      status.textContent = passed ? 'Brig file and network boundary checks passed.' : 'Some Brig boundaries could not be verified. Review the failed and inconclusive results.';
      if (!report.cleanup) status.textContent += ' Temporary files with an unverified identity were preserved.';
    } catch (error) { if (sameProjectView(current(), project) && checks.accepts(pending)) status.textContent = String(error); }
    finally { if (sameProjectView(current(), project) && checks.accepts(pending)) { checks.finish(pending); setBusy(run, false); run.disabled = false; } }
  });
  const copy = document.getElementById('copy-inner-boundary-prompt') as HTMLButtonElement;
  copy.addEventListener('click', async () => {
    const context = current(); if (!context) return;
    const pending = copying.begin();
    copy.disabled = true; setBusy(copy, true);
    try {
      await invoke('copy_inner_boundary_prompt');
      if (sameProjectView(current(), context) && copying.accepts(pending)) status.textContent = 'Check prompt copied. Paste it into the open conversation and review the results in the terminal.';
    } catch (error) { if (sameProjectView(current(), context) && copying.accepts(pending)) status.textContent = String(error); }
    finally { if (sameProjectView(current(), context) && copying.accepts(pending)) { copying.finish(pending); copy.disabled = false; setBusy(copy, false); } }
  });
  return () => { checks.invalidate(); copying.invalidate(); data.setState({ report: null }); run.disabled = false; copy.disabled = false; setBusy(run, false); setBusy(copy, false); status.textContent = ''; results.replaceChildren(); };
}
