const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { createManifest, verifyBundle, verifyProcesses, verifyRuntime } = require('../lib/rpi-artifact.cjs');
const repository = path.resolve(__dirname, '../..');

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dynamo-artifact-test-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, 'target/release'), { recursive: true });
  for (const name of ['dynamo-bootstrap', 'dynamo-dashboard', 'dynamo-bot']) {
    fs.writeFileSync(path.join(root, 'target/release', name), name);
  }
  return { root, manifest: createManifest(root, repository) };
}

const apps = () => [
  { name: 'dynamo-dashboard', pid: 10, pm2_env: { status: 'online' } },
  { name: 'dynamo-bot', pid: 20, pm2_env: { status: 'online' } },
];
const bytesForPid = pid => Buffer.from(pid === 10 ? 'dynamo-dashboard' : 'dynamo-bot');

test('manifest roundtrip and CLI validate exact build bytes and provenance', t => {
  const { root, manifest } = fixture(t);
  assert.deepEqual(verifyBundle(root), manifest);
  assert.equal(typeof manifest.dirty, 'boolean');
  const output = execFileSync(process.execPath, [path.join(repository, 'scripts/lib/rpi-artifact.cjs'), 'verify', root], { encoding: 'utf8' });
  assert.ok(output.includes(manifest.git_revision));
  assert.ok(output.includes(manifest.build_completed_at_utc));
});

test('missing manifest and replaced binary fail before upload', t => {
  const { root } = fixture(t);
  fs.writeFileSync(path.join(root, 'target/release/dynamo-bot'), 'older-bot');
  assert.throws(() => verifyBundle(root), /hash mismatch/);
  fs.unlinkSync(path.join(root, 'build-manifest.json'));
  assert.throws(() => verifyBundle(root), /ENOENT/);
});

test('unsupported manifest metadata fails closed', t => {
  const { root, manifest } = fixture(t);
  manifest.target = 'wrong-architecture';
  fs.writeFileSync(path.join(root, 'build-manifest.json'), JSON.stringify(manifest));
  assert.throws(() => verifyBundle(root), /Invalid deployment manifest/);
});

test('online status alone cannot hide a stale running binary', t => {
  const { manifest } = fixture(t);
  assert.equal(verifyProcesses(manifest, apps(), bytesForPid), 'dynamo-dashboard:10,dynamo-bot:20');
  assert.throws(() => verifyProcesses(manifest, apps(), () => Buffer.from('old')), /does not match/);
  const offline = apps();
  offline[1].pm2_env.status = 'errored';
  assert.throws(() => verifyProcesses(manifest, offline, bytesForPid), /Expected one online/);
  assert.throws(() => verifyProcesses(manifest, [...apps(), apps()[1]], bytesForPid), /Expected one online/);
});

test('runtime verification retries startup and requires stable matching pids', async t => {
  const { root } = fixture(t);
  let calls = 0;
  await verifyRuntime(root, {
    attempts: 3,
    readApps: () => (++calls === 1 ? [] : apps()),
    readExecutable: bytesForPid,
    wait: async () => {},
  });
  assert.equal(calls, 3);
});

test('runtime mismatch exhausts bounded retries and returns failure', async t => {
  const { root } = fixture(t);
  let calls = 0;
  await assert.rejects(verifyRuntime(root, {
    attempts: 3,
    readApps: () => { calls++; return apps(); },
    readExecutable: () => Buffer.from('old-bot'),
    wait: async () => {},
  }), /Deployment verification failed/);
  assert.equal(calls, 3);
});
