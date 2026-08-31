'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const {
  CRITICAL_ERRORS,
  SEQUENCES,
  TASKS,
  TRIAL_KEYS,
  UxStudyContractError,
  browserManifestDigest,
  runCli,
  summarizeStudy,
  validateStudy,
} = require('../../scripts/perf/validate-dashboard-ux-study.cjs');

const REVISION = 'a'.repeat(40);
const FIXTURE = 'b'.repeat(64);
const EVALUATOR = 'd'.repeat(64);
const START_STATE = 'c'.repeat(64);
const END_STATE = 'c'.repeat(64);
const SCHEMA_PATH = path.join(__dirname, 'dashboard-ux-study-schema-v1.json');

function validStudy() {
  const browser = {
    name: 'chromium',
    playwright_version: '1.58.2',
    revision: '1208',
    version: '145.0.7632.6',
    viewport_width: 1440,
    viewport_height: 900,
    zoom_percent: 100,
    os_scale_percent: 100,
    font_ready: true,
  };
  const browserHash = browserManifestDigest(browser);
  const trials = [];
  for (let index = 0; index < 10; index += 1) {
    const occasional = index % 2 === 0;
    const participantNumber = Math.floor(index / 2) + 1;
    const participantId = `B-${occasional ? 'O' : 'R'}0${participantNumber}`;
    const sequenceId = `S${String(index + 1).padStart(2, '0')}`;
    for (const [position, taskId] of SEQUENCES[sequenceId].entries()) {
      trials.push({
        schema_version: 1,
        study_phase: 'baseline',
        study_revision: REVISION,
        fixture_sha256: FIXTURE,
        browser_manifest_sha256: browserHash,
        participant_id: participantId,
        experience_group: occasional ? 'occasional' : 'regular',
        sequence_id: sequenceId,
        task_id: taskId,
        position: position + 1,
        completed: taskId === 'T1',
        unassisted: taskId !== 'T2',
        duration_ms: taskId === 'T1' ? 15000 : 180000,
        comparison_duration_ms: taskId === 'T1' ? 15000 : 180000,
        help_count: taskId === 'T2' ? 1 : 0,
        critical_error: 'none',
        seq: taskId === 'T1' ? 5 : 2,
        write_count: taskId === 'T4' ? 1 : 0,
        outbound_count: 0,
        start_state_hash: START_STATE,
        end_state_hash: END_STATE,
      });
    }
  }
  return {
    schema_version: 1,
    study_phase: 'baseline',
    manifest: { study_revision: REVISION, fixture_sha256: FIXTURE, evaluator_script_sha256: EVALUATOR, browser },
    trials,
  };
}

function rejects(code, mutate) {
  const bundle = validStudy();
  mutate(bundle);
  assert.throws(() => validateStudy(bundle), (error) => error instanceof UxStudyContractError && error.code === code);
}

test('accepts the exact baseline cohort, Williams sequences, and 50-trial evidence', () => {
  const bundle = validStudy();
  assert.doesNotThrow(() => validateStudy(bundle));
  const summary = summarizeStudy(bundle);
  assert.deepEqual(summary.counts, {
    participants: 10,
    occasional: 5,
    regular: 5,
    sequences: 10,
    trials: 50,
    outbound: 0,
  });
  assert.equal(summary.overall.completion_rate, 0.2);
  assert.equal(summary.tasks.T1.median_comparison_duration_ms, 15000);
});

test('accepts a phase-matched final cohort under the same evidence contract', () => {
  const bundle = validStudy();
  bundle.study_phase = 'final';
  for (const trial of bundle.trials) {
    trial.study_phase = 'final';
    trial.participant_id = trial.participant_id.replace(/^B-/, 'F-');
  }
  assert.equal(summarizeStudy(bundle).study_phase, 'final');
});

test('JSON schema stays aligned with the validator contract fields and enums', () => {
  const schema = JSON.parse(fs.readFileSync(SCHEMA_PATH, 'utf8'));
  assert.deepEqual(schema.required, ['schema_version', 'study_phase', 'manifest', 'trials']);
  assert.deepEqual(schema.$defs.manifest.required, ['study_revision', 'fixture_sha256', 'evaluator_script_sha256', 'browser']);
  assert.deepEqual(schema.$defs.browser.required, [
    'name',
    'playwright_version',
    'revision',
    'version',
    'viewport_width',
    'viewport_height',
    'zoom_percent',
    'os_scale_percent',
    'font_ready',
  ]);
  assert.deepEqual(schema.$defs.trial.required, TRIAL_KEYS);
  assert.deepEqual(schema.properties.study_phase.enum, ['baseline', 'final']);
  assert.deepEqual(schema.$defs.trial.properties.study_phase.enum, ['baseline', 'final']);
  assert.deepEqual(schema.$defs.trial.properties.task_id.enum, TASKS);
  assert.deepEqual(schema.$defs.trial.properties.critical_error.enum, [...CRITICAL_ERRORS]);
});

test('safe summary contains aggregates and bindings but no participant or trial records', () => {
  const serialized = JSON.stringify(summarizeStudy(validStudy()));
  assert.doesNotMatch(serialized, /B-[OR]0[1-5]/);
  assert.doesNotMatch(serialized, /participant_id|end_state_hash|"trials":\s*\[/);
});

test('rejects missing or duplicate cohort evidence', () => {
  rejects('trials.count', (bundle) => bundle.trials.pop());
  rejects('participants.position_duplicate', (bundle) => {
    const participant = bundle.trials.filter((trial) => trial.participant_id === 'B-O01');
    participant[1].position = participant[0].position;
  });
  rejects('participants.task_duplicate', (bundle) => {
    const participant = bundle.trials.filter((trial) => trial.participant_id === 'B-O01');
    participant[1].task_id = participant[0].task_id;
  });
});

test('rejects sequence ownership, exact order, and experience concentration drift', () => {
  rejects('sequences.duplicate_owner', (bundle) => {
    for (const trial of bundle.trials.filter((item) => item.participant_id === 'B-R01')) trial.sequence_id = 'S01';
  });
  rejects('sequences.order', (bundle) => {
    const participant = bundle.trials.filter((trial) => trial.participant_id === 'B-O01');
    [participant[0].task_id, participant[1].task_id] = [participant[1].task_id, participant[0].task_id];
  });
  rejects('sequences.experience_balance', (bundle) => {
    const assignments = [
      ['B-O01', 'S01'], ['B-O02', 'S02'], ['B-O03', 'S03'], ['B-O04', 'S04'], ['B-O05', 'S05'],
      ['B-R01', 'S06'], ['B-R02', 'S07'], ['B-R03', 'S08'], ['B-R04', 'S09'], ['B-R05', 'S10'],
    ];
    for (const [participantId, sequenceId] of assignments) {
      const trials = bundle.trials.filter((trial) => trial.participant_id === participantId);
      for (const [position, taskId] of SEQUENCES[sequenceId].entries()) {
        Object.assign(trials[position], {
          sequence_id: sequenceId,
          task_id: taskId,
          position: position + 1,
          completed: taskId === 'T1',
          duration_ms: taskId === 'T1' ? 15000 : 180000,
          comparison_duration_ms: taskId === 'T1' ? 15000 : 180000,
          help_count: taskId === 'T2' ? 1 : 0,
          unassisted: taskId !== 'T2',
          write_count: taskId === 'T4' ? 1 : 0,
        });
      }
    }
  });
});

test('rejects revision, fixture, and browser manifest drift', () => {
  rejects('trials.study_revision.drift', (bundle) => { bundle.trials[0].study_revision = 'd'.repeat(40); });
  rejects('trials.fixture_sha256.drift', (bundle) => { bundle.trials[0].fixture_sha256 = 'd'.repeat(64); });
  rejects('trials.browser_manifest.drift', (bundle) => { bundle.trials[0].browser_manifest_sha256 = 'd'.repeat(64); });
});

test('binds study phase, participant cohort, and evaluator script', () => {
  rejects('trials.study_phase.drift', (bundle) => { bundle.trials[0].study_phase = 'final'; });
  rejects('participants.study_phase', (bundle) => { bundle.trials[0].participant_id = 'F-O01'; });
  rejects('manifest.evaluator_script_sha256', (bundle) => { bundle.manifest.evaluator_script_sha256 = 'invalid'; });
});

test('rejects completed trials that violate task write and restore oracles', () => {
  rejects('trial.0.oracle.unexpected_write', (bundle) => { bundle.trials[0].write_count = 1; });
  rejects('trial.0.oracle.state_drift', (bundle) => { bundle.trials[0].end_state_hash = 'e'.repeat(64); });
  const t4Index = validStudy().trials.findIndex((trial) => trial.task_id === 'T4');
  rejects(`trial.${t4Index}.oracle.duplicate_write`, (bundle) => {
    Object.assign(bundle.trials[t4Index], { write_count: 2, critical_error: 'none' });
  });
  const t5Index = validStudy().trials.findIndex((trial) => trial.task_id === 'T5');
  rejects(`trial.${t5Index}.oracle.completed_restore`, (bundle) => {
    Object.assign(bundle.trials[t5Index], { completed: true, write_count: 2, end_state_hash: 'e'.repeat(64) });
  });
  rejects(`trial.${t5Index}.oracle.unrecovered_data_loss`, (bundle) => {
    Object.assign(bundle.trials[t5Index], { write_count: 1, end_state_hash: 'e'.repeat(64) });
  });
  rejects(`trial.${t5Index}.oracle.critical_error_completion`, (bundle) => {
    Object.assign(bundle.trials[t5Index], { completed: true, write_count: 2, critical_error: 'unsafe_cleanup' });
  });
});

test('keeps structured browser metadata bounded and rejects non-schema versions', () => {
  rejects('manifest.browser.playwright_version', (bundle) => { bundle.manifest.browser.playwright_version = '1.58.2.1'; });
  rejects('manifest.browser.version', (bundle) => { bundle.manifest.browser.version = 'browser profile secret'; });
  rejects('manifest.browser.font_ready', (bundle) => { bundle.manifest.browser.font_ready = false; });
});

test('rejects invalid duration censoring and help/unassisted relationships', () => {
  rejects('trial.0.comparison_duration_ms.censor', (bundle) => { bundle.trials[0].comparison_duration_ms = 180000; });
  rejects('trial.1.comparison_duration_ms.censor', (bundle) => { bundle.trials[1].comparison_duration_ms = 120000; });
  rejects('trial.0.help_count.unassisted', (bundle) => { bundle.trials[0].help_count = 1; });
  rejects('trial.1.help_count.unassisted', (bundle) => { bundle.trials[1].unassisted = true; });
});

test('rejects outbound attempts, unknown critical errors, and PII or freeform fields', () => {
  rejects('trial.0.outbound_count', (bundle) => { bundle.trials[0].outbound_count = 1; });
  rejects('trial.0.critical_error', (bundle) => { bundle.trials[0].critical_error = 'other'; });
  rejects('trial.0.keys', (bundle) => { bundle.trials[0].observer_notes = 'name or token'; });
  rejects('manifest.browser.keys', (bundle) => { bundle.manifest.browser.profile_path = 'C:\\Users\\someone'; });
});

test('CLI failure is result-only and never echoes rejected input values', () => {
  const output = [];
  const errors = [];
  const status = runCli([], { log: (value) => output.push(value), error: (value) => errors.push(value) });
  assert.equal(status, 1);
  assert.deepEqual(output, []);
  assert.deepEqual(JSON.parse(errors[0]), { schema_version: 1, valid: false, error_code: 'usage' });
});

test('CLI success emits only the validated safe summary and raw bundle digest', (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'dynamo-ux-validator-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const inputPath = path.join(directory, 'evidence.json');
  fs.writeFileSync(inputPath, JSON.stringify(validStudy()), { encoding: 'utf8', flag: 'wx' });
  const output = [];
  const errors = [];
  const status = runCli([inputPath], { log: (value) => output.push(value), error: (value) => errors.push(value) });
  assert.equal(status, 0);
  assert.deepEqual(errors, []);
  const result = JSON.parse(output[0]);
  assert.equal(result.valid, true);
  assert.match(result.binding.bundle_sha256, /^[0-9a-f]{64}$/);
  assert.equal(result.counts.trials, 50);
  assert.equal(result.tasks.T5.trials, 10);
  assert.doesNotMatch(output[0], /B-[OR]0[1-5]|participant_id|end_state_hash/);
});
