import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { assetNames, assertNewVersion, releaseManifest, verifyReleaseManifest, verifyUploadedAssets } from './macos-release-assets.mjs';
import { verifyMacosRelease } from './verify-macos-release.mjs';

function gh(args) {
  return execFileSync('gh', args, {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'inherit'],
    timeout: 300_000,
  }).trim();
}

export async function publishMacosRelease(
  { repository, version, buildNumber, commit, directory }, github = gh, verifyRelease = verifyMacosRelease,
) {
  if (repository !== 'reformo/tugboat') throw new Error('The production feed belongs to reformo/tugboat.');
  if (!/^[1-9]\d*$/.test(buildNumber) || !/^[0-9a-f]{40}$/.test(commit)) throw new Error('Expected a build number and a full commit SHA.');
  const names = assetNames(version);
  const [installer, archive, signature, manifest] = names;
  const tag = `v${version}-build.${buildNumber}`;
  const apiRoot = `repos/${repository}`;
  const api = (...args) => JSON.parse(github(['api', ...args]));
  const verifyFiles = async files => {
    verifyReleaseManifest(files, version, tag);
    await verifyRelease(path.join(files, installer), path.join(files, archive), version);
  };
  const verifyTagCommit = tagRequired => {
    const refs = api(`${apiRoot}/git/matching-refs/tags/${tag}`);
    const ref = refs.find(candidate => candidate.ref === `refs/tags/${tag}`);
    if (ref ? ref.object.type !== 'commit' || ref.object.sha !== commit : tagRequired) {
      throw new Error(`Tag ${tag} does not identify commit ${commit}.`);
    }
  };
  const verifyReleaseCommit = release => {
    if (release.tag_name !== tag || release.target_commitish !== commit || release.prerelease !== false) {
      throw new Error(`Release ${tag} does not identify commit ${commit} as a stable release.`);
    }
    verifyTagCommit(!release.draft);
  };
  const verifyAssets = async (release, files) => {
    const pages = api('--paginate', '--slurp', `${apiRoot}/releases/${release.id}/assets?per_page=100`);
    await verifyUploadedAssets(files, names, pages.flat());
  };

  const releases = api('--paginate', '--slurp', `${apiRoot}/releases?per_page=100`).flat();
  const existing = releases.find(release => release.tag_name === tag);
  if (existing) verifyReleaseCommit(existing);
  if (existing && !existing.draft) {
    const files = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-published-'));
    try {
      github(['release', 'download', tag, '--repo', repository, '--dir', files,
        ...names.flatMap(name => ['--pattern', name])]);
      await verifyAssets(existing, files);
      await verifyFiles(files);
      console.log(`Release ${tag} already contains the four verified assets for ${commit}.`);
      return;
    } finally {
      fs.rmSync(files, { recursive: true, force: true });
    }
  }

  assertNewVersion(version, releases.filter(release => !release.draft && !release.prerelease).map(release => release.tag_name));
  verifyTagCommit(false);
  const candidate = api(`${apiRoot}/commits/${commit}`);
  if (candidate.sha !== commit) throw new Error(`GitHub could not verify commit ${commit}.`);

  const draft = existing ?? api('--method', 'POST', `${apiRoot}/releases`,
    '-f', `tag_name=${tag}`, '-f', `target_commitish=${commit}`,
    '-f', `name=Tugboat ${version} (build ${buildNumber})`, '-F', 'draft=true', '-F', 'generate_release_notes=true');
  verifyReleaseCommit(draft);
  if (!draft.draft) throw new Error(`Release ${tag} must be a draft before upload.`);

  const signatureContent = fs.readFileSync(path.join(directory, signature), 'utf8');
  const notes = draft.body?.trim() || `Tugboat ${version}`;
  fs.writeFileSync(path.join(directory, manifest), JSON.stringify(releaseManifest(version, tag, notes, signatureContent), null, 2) + '\n');
  await verifyFiles(directory);
  github(['release', 'upload', tag, '--repo', repository, ...names.map(name => path.join(directory, name)), '--clobber']);
  const uploadedDraft = api(`${apiRoot}/releases/${draft.id}`);
  verifyReleaseCommit(uploadedDraft);
  if (!uploadedDraft.draft) throw new Error(`Release ${tag} must be a draft during asset verification.`);
  await verifyAssets(uploadedDraft, directory);

  const published = api('--method', 'PATCH', `${apiRoot}/releases/${draft.id}`, '-F', 'draft=false', '-f', 'make_latest=true');
  verifyReleaseCommit(published);
  if (published.draft) throw new Error(`Release ${tag} is still a draft.`);
  console.log(`Published Tugboat ${version} from ${commit} with four verified assets.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await publishMacosRelease({
    repository: process.env.GITHUB_REPOSITORY,
    version: process.argv[2],
    buildNumber: process.env.GITHUB_RUN_NUMBER,
    commit: process.env.GITHUB_SHA,
    directory: process.argv[3],
  });
}
