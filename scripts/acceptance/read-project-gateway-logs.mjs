import { lstatSync, realpathSync } from 'node:fs';
import { isIPv4 } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { stripVTControlCharacters } from 'node:util';
import { runAcceptanceProcess } from './acceptance-process.mjs';

const bundledBrig = fileURLToPath(new URL('../../.build/brig-runtime/bin/brig', import.meta.url));
const projectReference = /^[a-z0-9][a-z0-9._-]{0,63}@[a-z0-9][a-z0-9._-]{0,63}$/;
const lineLimit = 100;
const hiddenEntry = '[Gateway log details removed]';
const hullRecord = /^time="(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2}))" level=(info|warning|error|debug) msg="(.*)"(?: subsystem=hull)?$/;
const publishedForward = /^network-gateway: (?:published|withdrew) (?:udp:)?127\.0\.0\.1:(\d{1,5})=([\d.]+):(\d{1,5})$/;
const deniedConnection = /^egress: rejected (?:tcp|udp) from ([\d.]+) to ([\d.]+):(\d{1,5})$/;

function scratchEnvironment(path, brig) {
  if (!isAbsolute(path) || !isAbsolute(brig)) throw new Error('Use absolute paths.');
  const root = realpathSync(path);
  const directory = lstatSync(root);
  if (![tmpdir(), '/tmp'].some(base => root.startsWith(realpathSync(base) + sep))
      || !directory.isDirectory() || directory.uid !== process.getuid() || (directory.mode & 0o077) !== 0) {
    throw new Error('Use an owned, private temporary acceptance directory.');
  }
  const home = join(root, 'home');
  if (!lstatSync(home).isDirectory()) throw new Error('Use the acceptance directory with its own home.');
  return {
    HOME: home, PATH: `${dirname(brig)}:/usr/bin:/bin:/usr/sbin:/sbin`, TERM: 'dumb', DO_NOT_TRACK: '1',
    XDG_CONFIG_HOME: join(root, 'config'), XDG_CACHE_HOME: join(root, 'cache'),
    XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'xdg-state'),
    BRIG_STATE_DIR: join(root, 'state'), BRIG_PROFILE_DIR: join(root, 'profiles'),
    BRIG_POLICY_DIR: join(root, 'policies'), BRIG_NETWORK: 'isolated',
  };
}

function validPort(port) {
  return Number(port) > 0 && Number(port) <= 65_535;
}

function safeMessage(message) {
  const forward = publishedForward.exec(message);
  if (forward && validPort(forward[1]) && isIPv4(forward[2]) && validPort(forward[3])) return message;
  const denial = deniedConnection.exec(message);
  if (denial && isIPv4(denial[1]) && isIPv4(denial[2]) && validPort(denial[3])) return message;
  if (message.startsWith('egress: refused the ') && message.includes(' query ')) return 'egress: refused a DNS query';
  if (message.startsWith('egress: cannot resolve the allowed host ')) return 'egress: could not resolve an allowed host';
  if (message.startsWith('network-gateway on ')) return 'network-gateway: started';
  return '';
}

function displayEntry(line) {
  const record = hullRecord.exec(line);
  if (!record) return hiddenEntry;
  const [, timestamp, level, message] = record;
  // The tail may start inside a multiline secret. Only render known event fields.
  const summary = safeMessage(message);
  return summary ? `${timestamp} ${level} ${summary}` : hiddenEntry;
}

function safeLogText(stdout) {
  const text = stripVTControlCharacters(stdout).replaceAll('\r\n', '\n')
    .replace(/[\p{Cc}\p{Cf}]/gu, character => '\n\t'.includes(character) ? character : '');
  if (!text.trim()) return '';
  const lines = text.split('\n');
  if (lines.at(-1) === '') lines.pop();
  return lines.slice(-lineLimit).map(displayEntry).join('\n');
}

export async function readProjectGatewayLogs(root, reference, { brig = bundledBrig, timeout = 30_000 } = {}) {
  if (typeof reference !== 'string' || !projectReference.test(reference)) {
    throw new Error('Use the full Brig profile@session reference.');
  }
  const env = scratchEnvironment(root, brig);
  const result = await runAcceptanceProcess(brig, ['logs', '--gateway', reference, '--tail', String(lineLimit)], { env, cwd: env.HOME, timeout });
  if (result.error === 'ETIMEDOUT') return { status: 'timeout' };
  if (result.error === 'ENOBUFS') return { status: 'output-limit' };
  if (!result.error && result.status === 3) return { status: 'unavailable' };
  if (result.error || result.status !== 0) return { status: 'failed' };
  const text = safeLogText(result.stdout);
  return text ? { status: 'available', text } : { status: 'empty' };
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  const [root, reference, brig, ...extra] = process.argv.slice(2);
  try {
    if (!root || !reference || extra.length) throw new Error('Usage: read-project-gateway-logs.mjs <scratch-root> <profile@session> [<brig-path>]');
    const result = await readProjectGatewayLogs(root, reference, { brig });
    const messages = {
      unavailable: 'Gateway logs are unavailable for this project.',
      timeout: 'The gateway log read timed out.',
      'output-limit': 'Gateway output exceeded the size limit.',
      failed: 'Brig could not read the gateway log.',
    };
    if (result.status === 'available') console.log(result.text);
    if (result.status === 'empty') console.log('The project gateway log is empty.');
    if (messages[result.status]) {
      console.error(messages[result.status]);
      process.exitCode = 1;
    }
  } catch (error) {
    console.error(error.code ? 'Could not read the scratch acceptance directory.' : error.message);
    process.exitCode = 1;
  }
}
