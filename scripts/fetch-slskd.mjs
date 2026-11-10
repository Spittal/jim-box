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
  'win32-x64': {
    name: 'win-x64',
    sha256: '942299d8c97da6cc1f6cd82dcd4a3662b97b82fbd1742df4bec165b79357268a',
  },
  'win32-arm64': {
    name: 'win-arm64',
    sha256: '707ca26de16835d83ff980d9fee5a2c2fd8ec7bf8260be63d60805914a627e9d',
  },
};

// The platform being built for. Tauri tells its before-build commands the
// target, which differs from this machine's when cross-compiling (e.g. the
// Windows ARM64 build made on an x64 machine).
function target() {
  const platform = { windows: 'win32', darwin: 'darwin' }[process.env.TAURI_ENV_PLATFORM] ?? process.platform;
  const arch = { x86_64: 'x64', aarch64: 'arm64' }[process.env.TAURI_ENV_ARCH] ?? process.arch;
  return `${platform}-${arch}`;
}

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const destDir = join(root, 'src-tauri', 'resources', 'slskd');
const versionFile = join(destDir, 'VERSION');

const asset = ASSETS[target()];
if (!asset) {
  console.error(`fetch-slskd: no pinned slskd build for ${target()}`);
  process.exit(1);
}

// Names the build as well as the version, so switching target fetches again.
const stamp = `${SLSKD_VERSION} ${asset.name}`;
if (existsSync(versionFile) && readFileSync(versionFile, 'utf8').trim() === stamp) {
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
if (process.platform === 'win32') {
  // Windows has no `unzip`, but its bundled tar (bsdtar) reads zip files.
  // Named in full so that Git Bash's GNU tar, which can't, isn't picked up.
  const tar = join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe');
  execFileSync(tar, ['-xf', zipPath, '-C', destDir], { stdio: 'inherit' });
} else {
  // `unzip` preserves the executable bit on the slskd binary.
  execFileSync('unzip', ['-q', zipPath, '-d', destDir], { stdio: 'inherit' });
}
rmSync(zipPath);
writeFileSync(versionFile, `${stamp}\n`);

console.log(`fetch-slskd: slskd ${stamp} ready in ${destDir}`);
