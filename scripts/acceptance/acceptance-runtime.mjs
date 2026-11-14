import path from 'node:path';

export function runtimeConfigurationFile(runtime) {
  const directory = path.dirname(runtime);
  if (path.basename(directory) === 'MacOS') return path.resolve(directory, '../Resources/runtime/mke2fs.conf');
  return path.resolve(directory, '../mke2fs.conf');
}
