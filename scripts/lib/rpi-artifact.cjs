const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');

const binaries = ['dynamo-bootstrap', 'dynamo-dashboard', 'dynamo-bot'];
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');

function createManifest(root, repository) {
  const git = args => execFileSync('git', ['-C', repository, ...args], { encoding: 'utf8' }).trim();
  const manifest = {
    schema: 1,
    target: 'aarch64-unknown-linux-gnu',
    build_completed_at_utc: new Date().toISOString(),
    git_revision: git(['rev-parse', 'HEAD']),
    dirty: git(['status', '--porcelain']).length !== 0,
    binaries: Object.fromEntries(binaries.map(name => {
      const bytes = fs.readFileSync(path.join(root, 'target', 'release', name));
      return [name, { sha256: hash(bytes), size: bytes.length }];
    })),
  };
  fs.writeFileSync(path.join(root, 'build-manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
  return manifest;
}

function verifyBundle(root) {
  const manifest = JSON.parse(fs.readFileSync(path.join(root, 'build-manifest.json'), 'utf8'));
  if (manifest.schema !== 1 || manifest.target !== 'aarch64-unknown-linux-gnu'
      || !/^[0-9a-f]{40,64}$/.test(manifest.git_revision)
      || typeof manifest.dirty !== 'boolean' || !Number.isFinite(Date.parse(manifest.build_completed_at_utc))) {
    throw new Error('Invalid deployment manifest metadata');
  }
  for (const name of binaries) {
    const entry = manifest.binaries?.[name];
    const file = path.join(root, 'target', 'release', name);
    if (!fs.lstatSync(file).isFile() || !entry || !/^[0-9a-f]{64}$/.test(entry.sha256)) {
      throw new Error(`Invalid deployment binary entry: ${name}`);
    }
    const bytes = fs.readFileSync(file);
    if (entry.size !== bytes.length || entry.sha256 !== hash(bytes)) {
      throw new Error(`Deployment binary hash mismatch: ${name}`);
    }
  }
  return manifest;
}

function verifyProcesses(manifest, apps, readExecutable = pid => fs.readFileSync(`/proc/${pid}/exe`)) {
  return ['dynamo-dashboard', 'dynamo-bot'].map(name => {
    const matches = apps.filter(app => app.name === name);
    if (matches.length !== 1 || matches[0].pm2_env?.status !== 'online'
        || !Number.isSafeInteger(matches[0].pid) || matches[0].pid <= 0) {
      throw new Error(`Expected one online process: ${name}`);
    }
    const pid = matches[0].pid;
    if (hash(readExecutable(pid)) !== manifest.binaries[name].sha256) {
      throw new Error(`Running executable does not match deployment: ${name} pid=${pid}`);
    }
    return `${name}:${pid}`;
  }).join(',');
}

async function verifyRuntime(root, {
  attempts = 30,
  readApps = () => JSON.parse(execFileSync('pm2', ['jlist'], { encoding: 'utf8', timeout: 5000, maxBuffer: 8 * 1024 * 1024 })),
  readExecutable,
  wait = () => new Promise(resolve => setTimeout(resolve, 1000)),
} = {}) {
  const manifest = verifyBundle(root);
  const deadline = Date.now() + 30_000;
  let previous = null;
  let lastError = 'Processes did not stabilize';
  for (let attempt = 0; attempt < attempts && Date.now() < deadline; attempt++) {
    try {
      const current = verifyProcesses(manifest, readApps(), readExecutable);
      if (current === previous) {
        console.log(`Verified running deployment: ${current}; revision=${manifest.git_revision}; built=${manifest.build_completed_at_utc}; dirty=${manifest.dirty}`);
        return;
      }
      previous = current;
    } catch (error) {
      previous = null;
      // Do not print PM2 output or environment variables from child errors.
      lastError = error.message.startsWith('Expected one') || error.message.startsWith('Running executable')
        ? error.message : 'Unable to inspect PM2 processes or executable bytes';
    }
    await wait();
  }
  throw new Error(`Deployment verification failed: ${lastError}`);
}

async function main() {
  const [mode, root, repository] = process.argv.slice(2);
  if (!root || !['create', 'verify', 'running'].includes(mode)) {
    throw new Error('Usage: node rpi-artifact.cjs create|verify|running ROOT [REPOSITORY]');
  }
  if (mode === 'running') return verifyRuntime(root);
  const manifest = mode === 'create' ? createManifest(root, repository || root) : verifyBundle(root);
  console.log(`Verified bundle: revision=${manifest.git_revision}; built=${manifest.build_completed_at_utc}; dirty=${manifest.dirty}`);
}

module.exports = { createManifest, verifyBundle, verifyProcesses, verifyRuntime };
if (require.main === module) main().catch(error => { console.error(error.message); process.exitCode = 1; });
