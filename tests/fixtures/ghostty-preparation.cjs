const fs = require('node:fs');
const path = require('node:path');
const childProcess = require('node:child_process');
const execute = childProcess.execFileSync;

childProcess.execFileSync = (binary, argumentsList, options) => {
  fs.appendFileSync(process.env.TUGBOAT_GHOSTTY_COMMANDS, JSON.stringify({ binary, argumentsList, timeout: options?.timeout }) + '\n');
  if (binary === '/usr/bin/curl') return execute(binary, argumentsList, options);
  if (binary === '/usr/bin/tar') fs.mkdirSync(path.join(process.cwd(), '.build/libghostty/libghostty-vt-fixture'), { recursive: true });
  if (path.basename(binary) === 'zig') {
    const library = path.join(process.cwd(), '.build/libghostty/install/lib/libghostty-vt.a');
    fs.mkdirSync(path.dirname(library), { recursive: true });
    fs.writeFileSync(library, 'fixture library');
  }
  return Buffer.alloc(0);
};
require('node:module').syncBuiltinESMExports();
