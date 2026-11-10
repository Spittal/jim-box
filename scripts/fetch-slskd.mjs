// Fetches the pinned slskd release into src-tauri/resources/slskd so Tauri can
// bundle it. Idempotent: skips the download when the pinned version is present.
//
// To upgrade slskd: bump SLSKD_VERSION and replace each checksum with the
// SHA-256 of the corresponding release zip.

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SLSKD_VERSION = '0.26.0';

const ASSETS = {
  'darwin-arm64': {
    name: 'osx-arm64',
    sha256: '53bd82e26224908abb30780f3e3a3ee58788d17379354b3138c85c6fe02cd5a0',
  },
  'darwin-x64': {
    name: 'osx-x64',
    sha256: '3d624c53de73229caa090c395ee5eada9c7f54d59fd3a0e79a2597e8b467b448',
  },
};

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const destDir = join(root, 'src-tauri', 'resources', 'slskd');
const versionFile = join(destDir, 'VERSION');

const asset = ASSETS[`${process.platform}-${process.arch}`];
if (!asset) {
  console.error(`fetch-slskd: no pinned slskd build for ${process.platform}-${process.arch}`);
  process.exit(1);
}

if (existsSync(versionFile) && readFileSync(versionFile, 'utf8').trim() === SLSKD_VERSION) {
  process.exit(0);
}

const url = `https://github.com/slskd/slskd/releases/download/${SLSKD_VERSION}/slskd-${SLSKD_VERSION}-${asset.name}.zip`;
console.log(`fetch-slskd: downloading ${url}`);

const res = await fetch(url);
if (!res.ok) {
  console.error(`fetch-slskd: download failed with HTTP ${res.status}`);
  process.exit(1);
}
const zip = Buffer.from(await res.arrayBuffer());

const actual = createHash('sha256').update(zip).digest('hex');
if (actual !== asset.sha256) {
  console.error(`fetch-slskd: checksum mismatch (expected ${asset.sha256}, got ${actual})`);
  process.exit(1);
}

const zipPath = join(tmpdir(), `slskd-${SLSKD_VERSION}-${asset.name}.zip`);
writeFileSync(zipPath, zip);

rmSync(destDir, { recursive: true, force: true });
mkdirSync(destDir, { recursive: true });
// `unzip` preserves the executable bit on the slskd binary.
execFileSync('unzip', ['-q', zipPath, '-d', destDir], { stdio: 'inherit' });
rmSync(zipPath);
writeFileSync(versionFile, `${SLSKD_VERSION}\n`);

console.log(`fetch-slskd: slskd ${SLSKD_VERSION} ready in ${destDir}`);
