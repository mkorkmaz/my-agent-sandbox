import fs from 'node:fs';
import path from 'node:path';
import { createHash, createPublicKey, verify } from 'node:crypto';

const stableVersion = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const releaseTag = /^v(.+?)(?:-build\.[1-9]\d*)?$/;
const archiveName = 'Tugboat.app.tar.gz';
const platform = 'darwin-aarch64';

export function assetNames(version) {
  if (!stableVersion.test(version)) throw new Error('The app version must be a stable version in major.minor.patch form.');
  return [`Tugboat_${version}_aarch64.dmg`, archiveName, archiveName + '.sig', 'latest.json'];
}

export function assertNewVersion(version, publishedTags) {
  assetNames(version);
  const current = version.split('.').map(BigInt);
  for (const tag of publishedTags) {
    const published = releaseTag.exec(tag)?.[1];
    if (!published || !stableVersion.test(published)) continue;
    const previous = published.split('.').map(BigInt);
    const firstDifference = current.findIndex((part, index) => part !== previous[index]);
    if (firstDifference === -1 || current[firstDifference] < previous[firstDifference]) {
      throw new Error(`App version ${version} must be higher than the public version in ${tag}.`);
    }
  }
}

export function releaseManifest(version, tag, notes, signature) {
  return {
    version,
    notes,
    platforms: {
      [platform]: {
        url: `https://github.com/reformo/tugboat/releases/download/${tag}/${archiveName}`,
        signature,
      },
    },
  };
}

export function verifyReleaseManifest(directory, version, tag) {
  const uploaded = JSON.parse(fs.readFileSync(path.join(directory, 'latest.json'), 'utf8'));
  const signature = fs.readFileSync(path.join(directory, archiveName + '.sig'), 'utf8');
  if (typeof uploaded?.notes !== 'string' || !uploaded.notes.trim()) throw new Error('Release notes are missing from latest.json.');
  if (Buffer.byteLength(uploaded.notes, 'utf8') > 64 * 1024) throw new Error('Release notes exceed the updater limit of 64 KiB.');
  const expected = releaseManifest(version, tag, uploaded.notes, signature);
  if (uploaded.version !== version ||
      Object.keys(uploaded.platforms ?? {}).join() !== platform ||
      uploaded.platforms[platform]?.url !== expected.platforms[platform].url ||
      uploaded.platforms[platform]?.signature !== signature) {
    throw new Error('latest.json does not match the release version, archive URL and signature file.');
  }
}

export async function fileHash(file, algorithm) {
  const hash = createHash(algorithm);
  for await (const chunk of fs.createReadStream(file)) hash.update(chunk);
  return hash.digest();
}

export async function verifyUploadedAssets(directory, names, assets) {
  if (assets.length !== names.length) throw new Error('Expected exactly four release assets.');
  for (const name of names) {
    const asset = assets.find(candidate => candidate.name === name);
    const digest = 'sha256:' + (await fileHash(path.join(directory, name), 'sha256')).toString('hex');
    if (asset?.state !== 'uploaded' || asset.digest !== digest) {
      throw new Error(`Uploaded asset ${name} does not match the verified local file.`);
    }
  }
}

function decodeBase64(input) {
  const decoded = Buffer.from(input, 'base64');
  if (!decoded.length || decoded.toString('base64') !== input) throw new Error('Invalid Tauri updater signature or public key encoding.');
  return decoded;
}

export async function verifyUpdaterArchive(archive, version, publicKey) {
  // Tauri wraps a minisign public key and signature in base64.
  const [, key] = decodeBase64(publicKey).toString('utf8').split('\n');
  const [, signature, comment, commentSignature] =
    decodeBase64(fs.readFileSync(archive + '.sig', 'utf8')).toString('utf8').split('\n');
  const publicBytes = decodeBase64(key ?? '');
  const signatureBytes = decodeBase64(signature ?? '');
  const prefix = 'trusted comment: ';
  if (publicBytes.length !== 42 || publicBytes.subarray(0, 2).toString() !== 'Ed' ||
      signatureBytes.length !== 74 || signatureBytes.subarray(0, 2).toString() !== 'ED' ||
      !signatureBytes.subarray(2, 10).equals(publicBytes.subarray(2, 10)) ||
      !comment?.startsWith(prefix)) {
    throw new Error('Invalid Tauri updater signature or public key.');
  }

  const verificationKey = createPublicKey({
    key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), publicBytes.subarray(10)]),
    format: 'der',
    type: 'spki',
  });
  const signatureData = signatureBytes.subarray(10);
  const digest = await fileHash(archive, 'blake2b512');
  if (!verify(null, digest, verificationKey, signatureData) ||
      !verify(null, Buffer.concat([signatureData, Buffer.from(comment.slice(prefix.length))]), verificationKey, decodeBase64(commentSignature ?? ''))) {
    throw new Error('Invalid Tauri updater archive signature.');
  }
  if (comment.slice(prefix.length).split('\t').find(field => field.startsWith('version:')) !== `version:${version}`) {
    throw new Error(`The updater signature does not bind the archive to version ${version}.`);
  }
}
