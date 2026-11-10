import { invoke } from '@tauri-apps/api/core';

interface Probe { name: string; target: string; expected: string; observed: string; outcome: string }
interface Report { projectId: string; planFingerprint: string; passed: boolean; cleanup: boolean; results: Probe[] }

export function resetBoundaryChoice(): void {
  const choice = document.getElementById('use-brig-boundary') as HTMLInputElement;
  choice.checked = false;
  choice.disabled = true;
}

export function initializeBoundaryTests(current: () => { id: string; fingerprint: string | null } | null): void {
  const run = document.getElementById('run-boundary-tests') as HTMLButtonElement;
  const status = document.getElementById('boundary-status')!;
  const results = document.getElementById('boundary-results')!;
  run.addEventListener('click', async () => {
    const project = current();
    if (!project) return;
    run.disabled = true;
    status.textContent = 'Checking file sharing and network boundaries…';
    resetBoundaryChoice();
    try {
      const report = await invoke<Report>('run_boundary_tests', { identifier: project.id });
      if (current()?.id !== report.projectId) return;
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
      status.textContent = report.passed ? 'Brig file and network boundary checks passed. Check the inner Codex sandbox in the same conversation.' : 'Some Brig boundaries could not be verified. Review the failed and inconclusive results.';
      if (!report.cleanup) status.textContent += ' Temporary files with an unverified identity were preserved.';
      const choice = document.getElementById('use-brig-boundary') as HTMLInputElement;
      choice.disabled = !(report.passed && report.planFingerprint === current()?.fingerprint);
    } catch (error) { if (current()?.id === project.id) status.textContent = String(error); }
    finally { run.disabled = false; }
  });
  document.getElementById('copy-inner-boundary-prompt')!.addEventListener('click', async () => {
    await invoke('copy_inner_boundary_prompt');
    status.textContent = 'Check prompt copied. Paste it into the open Codex conversation and review the results in the terminal.';
  });
}
