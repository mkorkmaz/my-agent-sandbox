import { spawn } from 'node:child_process';

export function runAcceptanceProcess(binary, args, { env, cwd, timeout, maxBuffer = 1_048_576 }) {
  return new Promise(resolve => {
    const result = { status: null, signal: null, error: null, stdout: '', stderr: '' };
    const child = spawn(binary, args, { env, cwd, detached: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let outputBytes = 0;
    let completed = false;
    const finish = (status, signal) => {
      if (completed) return;
      completed = true;
      clearTimeout(timer);
      resolve({ ...result, status, signal });
    };
    const stop = code => {
      result.error ??= code;
      if (!child.pid) return;
      try { process.kill(-child.pid, 'SIGKILL'); }
      catch (error) {
        if (error.code === 'ESRCH') return;
        result.terminationError = error.code;
        child.stdout.destroy(); child.stderr.destroy(); child.unref();
        finish(null, null);
      }
    };
    const timer = setTimeout(() => stop('ETIMEDOUT'), timeout);
    for (const name of ['stdout', 'stderr']) {
      child[name].setEncoding('utf8');
      child[name].on('data', text => {
        outputBytes += Buffer.byteLength(text);
        if (outputBytes > maxBuffer) { stop('ENOBUFS'); return; }
        result[name] += text;
      });
    }
    child.once('error', error => { result.error = error.code; });
    child.once('close', finish);
  });
}
