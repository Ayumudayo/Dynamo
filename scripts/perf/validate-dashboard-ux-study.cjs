'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');

const MAX_EVIDENCE_BYTES = 2 * 1024 * 1024;
const TASKS = Object.freeze(['T1', 'T2', 'T3', 'T4', 'T5']);
const SEQUENCES = Object.freeze({
  S01: Object.freeze(['T1', 'T2', 'T5', 'T3', 'T4']),
  S02: Object.freeze(['T4', 'T3', 'T5', 'T2', 'T1']),
  S03: Object.freeze(['T2', 'T3', 'T1', 'T4', 'T5']),
  S04: Object.freeze(['T5', 'T4', 'T1', 'T3', 'T2']),
  S05: Object.freeze(['T3', 'T4', 'T2', 'T5', 'T1']),
  S06: Object.freeze(['T1', 'T5', 'T2', 'T4', 'T3']),
  S07: Object.freeze(['T4', 'T5', 'T3', 'T1', 'T2']),
  S08: Object.freeze(['T2', 'T1', 'T3', 'T5', 'T4']),
  S09: Object.freeze(['T5', 'T1', 'T4', 'T2', 'T3']),
  S10: Object.freeze(['T3', 'T2', 'T4', 'T1', 'T5']),
});
const CRITICAL_ERRORS = new Set([
  'none',
  'false_success',
  'wrong_scope',
  'duplicate_write',
  'unsafe_cleanup',
  'unrecovered_data_loss',
]);
const HEX_40 = /^[0-9a-f]{40}$/;
const HEX_64 = /^[0-9a-f]{64}$/;
const PARTICIPANT = /^([BF])-([OR])0([1-5])$/;
const PLAYWRIGHT_VERSION = /^[0-9]+\.[0-9]+\.[0-9]+$/;
const SAFE_VERSION = /^[0-9]+(?:\.[0-9]+){2,3}$/;
const SAFE_REVISION = /^[0-9]{1,10}$/;

class UxStudyContractError extends Error {
  constructor(code) {
    super(code);
    this.name = 'UxStudyContractError';
    this.code = code;
  }
}

function fail(code) {
  throw new UxStudyContractError(code);
}

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value) && Object.getPrototypeOf(value) === Object.prototype;
}

function assertExactKeys(value, expected, code) {
  if (!isPlainObject(value)) fail(`${code}.type`);
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) fail(`${code}.keys`);
}

function assertInteger(value, minimum, maximum, code) {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) fail(code);
}

function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (isPlainObject(value)) {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

function sha256(value) {
  return crypto.createHash('sha256').update(value).digest('hex');
}

function browserManifestDigest(browser) {
  validateBrowser(browser);
  return sha256(canonicalJson(browser));
}

function validateBrowser(browser) {
  assertExactKeys(browser, [
    'name',
    'playwright_version',
    'revision',
    'version',
    'viewport_width',
    'viewport_height',
    'zoom_percent',
    'os_scale_percent',
    'font_ready',
  ], 'manifest.browser');
  if (browser.name !== 'chromium') fail('manifest.browser.name');
  if (typeof browser.playwright_version !== 'string' || !PLAYWRIGHT_VERSION.test(browser.playwright_version) || browser.playwright_version.length > 32) fail('manifest.browser.playwright_version');
  if (typeof browser.revision !== 'string' || !SAFE_REVISION.test(browser.revision)) fail('manifest.browser.revision');
  if (typeof browser.version !== 'string' || !SAFE_VERSION.test(browser.version) || browser.version.length > 32) fail('manifest.browser.version');
  assertInteger(browser.viewport_width, 320, 7680, 'manifest.browser.viewport_width');
  assertInteger(browser.viewport_height, 320, 4320, 'manifest.browser.viewport_height');
  assertInteger(browser.zoom_percent, 50, 200, 'manifest.browser.zoom_percent');
  assertInteger(browser.os_scale_percent, 100, 300, 'manifest.browser.os_scale_percent');
  if (browser.font_ready !== true) fail('manifest.browser.font_ready');
}

const TRIAL_KEYS = Object.freeze([
  'schema_version', 'study_phase', 'study_revision', 'fixture_sha256', 'browser_manifest_sha256',
  'participant_id', 'experience_group', 'sequence_id', 'task_id', 'position', 'completed',
  'unassisted', 'duration_ms', 'comparison_duration_ms', 'help_count', 'critical_error', 'seq',
  'write_count', 'outbound_count', 'start_state_hash', 'end_state_hash',
]);

function validateTrialShape(trial, index) {
  const prefix = `trial.${index}`;
  assertExactKeys(trial, TRIAL_KEYS, prefix);
  if (trial.schema_version !== 1) fail(`${prefix}.schema_version`);
  if (trial.study_phase !== 'baseline' && trial.study_phase !== 'final') fail(`${prefix}.study_phase`);
  if (!HEX_40.test(trial.study_revision)) fail(`${prefix}.study_revision`);
  if (!HEX_64.test(trial.fixture_sha256)) fail(`${prefix}.fixture_sha256`);
  if (!HEX_64.test(trial.browser_manifest_sha256)) fail(`${prefix}.browser_manifest_sha256`);
  if (typeof trial.participant_id !== 'string' || !PARTICIPANT.test(trial.participant_id)) fail(`${prefix}.participant_id`);
  if (trial.experience_group !== 'occasional' && trial.experience_group !== 'regular') fail(`${prefix}.experience_group`);
  if (!Object.hasOwn(SEQUENCES, trial.sequence_id)) fail(`${prefix}.sequence_id`);
  if (!TASKS.includes(trial.task_id)) fail(`${prefix}.task_id`);
  assertInteger(trial.position, 1, 5, `${prefix}.position`);
  if (typeof trial.completed !== 'boolean') fail(`${prefix}.completed`);
  if (typeof trial.unassisted !== 'boolean') fail(`${prefix}.unassisted`);
  assertInteger(trial.duration_ms, 1, 180000, `${prefix}.duration_ms`);
  assertInteger(trial.comparison_duration_ms, 1, 180000, `${prefix}.comparison_duration_ms`);
  assertInteger(trial.help_count, 0, 1, `${prefix}.help_count`);
  if (!CRITICAL_ERRORS.has(trial.critical_error)) fail(`${prefix}.critical_error`);
  assertInteger(trial.seq, 1, 7, `${prefix}.seq`);
  assertInteger(trial.write_count, 0, Number.MAX_SAFE_INTEGER, `${prefix}.write_count`);
  if (trial.outbound_count !== 0) fail(`${prefix}.outbound_count`);
  if (!HEX_64.test(trial.start_state_hash)) fail(`${prefix}.start_state_hash`);
  if (!HEX_64.test(trial.end_state_hash)) fail(`${prefix}.end_state_hash`);
  if (trial.comparison_duration_ms !== (trial.completed ? trial.duration_ms : 180000)) fail(`${prefix}.comparison_duration_ms.censor`);
  if (trial.unassisted !== (trial.help_count === 0)) fail(`${prefix}.help_count.unassisted`);
}

function validateTaskOracle(trial, index) {
  const prefix = `trial.${index}.oracle`;
  const restored = trial.start_state_hash === trial.end_state_hash;
  if (trial.critical_error !== 'none' && trial.completed) fail(`${prefix}.critical_error_completion`);
  if (trial.task_id === 'T1' || trial.task_id === 'T2' || trial.task_id === 'T3') {
    if (trial.write_count !== 0) fail(`${prefix}.unexpected_write`);
    if (!restored) fail(`${prefix}.state_drift`);
  }
  if (trial.task_id === 'T4') {
    if (trial.write_count > 1 && (trial.critical_error !== 'duplicate_write' || trial.completed)) fail(`${prefix}.duplicate_write`);
    if (trial.completed && trial.write_count !== 1) fail(`${prefix}.completed_write_count`);
  }
  if (trial.task_id === 'T5') {
    if (trial.write_count > 2) fail(`${prefix}.write_count`);
    if (trial.completed && trial.write_count !== 2) fail(`${prefix}.completed_write_count`);
    if (trial.completed && !restored) fail(`${prefix}.completed_restore`);
    if (!restored && (trial.critical_error !== 'unrecovered_data_loss' || trial.completed)) fail(`${prefix}.unrecovered_data_loss`);
  }
}

function median(values) {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function rate(numerator, denominator) {
  return Number((numerator / denominator).toFixed(4));
}

function validateStudy(bundle) {
  assertExactKeys(bundle, ['schema_version', 'study_phase', 'manifest', 'trials'], 'bundle');
  if (bundle.schema_version !== 1) fail('bundle.schema_version');
  if (bundle.study_phase !== 'baseline' && bundle.study_phase !== 'final') fail('bundle.study_phase');
  assertExactKeys(bundle.manifest, ['study_revision', 'fixture_sha256', 'evaluator_script_sha256', 'browser'], 'manifest');
  if (!HEX_40.test(bundle.manifest.study_revision)) fail('manifest.study_revision');
  if (!HEX_64.test(bundle.manifest.fixture_sha256)) fail('manifest.fixture_sha256');
  if (!HEX_64.test(bundle.manifest.evaluator_script_sha256)) fail('manifest.evaluator_script_sha256');
  const browserDigest = browserManifestDigest(bundle.manifest.browser);
  if (!Array.isArray(bundle.trials) || bundle.trials.length !== 50) fail('trials.count');

  const participants = new Map();
  const sequenceOwners = new Map();
  for (const [index, trial] of bundle.trials.entries()) {
    validateTrialShape(trial, index);
    validateTaskOracle(trial, index);
    if (trial.study_phase !== bundle.study_phase) fail('trials.study_phase.drift');
    if (trial.study_revision !== bundle.manifest.study_revision) fail('trials.study_revision.drift');
    if (trial.fixture_sha256 !== bundle.manifest.fixture_sha256) fail('trials.fixture_sha256.drift');
    if (trial.browser_manifest_sha256 !== browserDigest) fail('trials.browser_manifest.drift');
    const participantMatch = PARTICIPANT.exec(trial.participant_id);
    const expectedPrefix = bundle.study_phase === 'baseline' ? 'B' : 'F';
    if (participantMatch[1] !== expectedPrefix) fail('participants.study_phase');
    const expectedGroup = participantMatch[2] === 'O' ? 'occasional' : 'regular';
    if (trial.experience_group !== expectedGroup) fail('participants.experience_group');

    const existing = participants.get(trial.participant_id);
    if (existing && (existing.experience_group !== trial.experience_group || existing.sequence_id !== trial.sequence_id)) fail('participants.assignment.drift');
    if (!existing) participants.set(trial.participant_id, { experience_group: trial.experience_group, sequence_id: trial.sequence_id, trials: [] });
    participants.get(trial.participant_id).trials.push(trial);

    const owner = sequenceOwners.get(trial.sequence_id);
    if (owner !== undefined && owner !== trial.participant_id) fail('sequences.duplicate_owner');
    sequenceOwners.set(trial.sequence_id, trial.participant_id);
  }

  if (participants.size !== 10) fail('participants.count');
  const participantPrefix = bundle.study_phase === 'baseline' ? 'B' : 'F';
  const expectedIds = [...Array(5)].flatMap((_, index) => [`${participantPrefix}-O0${index + 1}`, `${participantPrefix}-R0${index + 1}`]).sort();
  if ([...participants.keys()].sort().some((id, index) => id !== expectedIds[index])) fail('participants.identity_set');
  if (sequenceOwners.size !== 10) fail('sequences.count');

  const groupBySequence = [];
  const pairCounts = new Map();
  for (const [sequenceId, expectedTasks] of Object.entries(SEQUENCES)) {
    const owner = sequenceOwners.get(sequenceId);
    if (!owner) fail('sequences.missing');
    const participant = participants.get(owner);
    const ordered = [...participant.trials].sort((left, right) => left.position - right.position);
    if (ordered.length !== 5) fail('participants.trial_count');
    if (new Set(ordered.map((trial) => trial.position)).size !== 5) fail('participants.position_duplicate');
    if (new Set(ordered.map((trial) => trial.task_id)).size !== 5) fail('participants.task_duplicate');
    if (ordered.some((trial, index) => trial.task_id !== expectedTasks[index] || trial.position !== index + 1)) fail('sequences.order');
    groupBySequence.push(participant.experience_group);
    for (let index = 0; index < expectedTasks.length - 1; index += 1) {
      const pair = `${expectedTasks[index]}>${expectedTasks[index + 1]}`;
      pairCounts.set(pair, (pairCounts.get(pair) || 0) + 1);
    }
  }
  const alternatingA = groupBySequence.every((group, index) => group === (index % 2 === 0 ? 'occasional' : 'regular'));
  const alternatingB = groupBySequence.every((group, index) => group === (index % 2 === 0 ? 'regular' : 'occasional'));
  if (!alternatingA && !alternatingB) fail('sequences.experience_balance');
  for (const first of TASKS) {
    for (const second of TASKS) {
      if (first !== second && pairCounts.get(`${first}>${second}`) !== 2) fail('sequences.ordered_pair_balance');
    }
  }

  return { browser_manifest_sha256: browserDigest };
}

function summarizeStudy(bundle, bundleSha256 = undefined) {
  const validated = validateStudy(bundle);
  const taskMetrics = {};
  for (const taskId of TASKS) {
    const trials = bundle.trials.filter((trial) => trial.task_id === taskId);
    const completed = trials.filter((trial) => trial.completed).length;
    const unassisted = trials.filter((trial) => trial.unassisted).length;
    taskMetrics[taskId] = {
      trials: trials.length,
      completion_rate: rate(completed, trials.length),
      unassisted_rate: rate(unassisted, trials.length),
      median_comparison_duration_ms: median(trials.map((trial) => trial.comparison_duration_ms)),
      help_total: trials.reduce((sum, trial) => sum + trial.help_count, 0),
      critical_error_total: trials.filter((trial) => trial.critical_error !== 'none').length,
      median_seq: median(trials.map((trial) => trial.seq)),
    };
  }
  const completed = bundle.trials.filter((trial) => trial.completed).length;
  const unassisted = bundle.trials.filter((trial) => trial.unassisted).length;
  const result = {
    schema_version: 1,
    valid: true,
    study_phase: bundle.study_phase,
    binding: {
      study_revision: bundle.manifest.study_revision,
      fixture_sha256: bundle.manifest.fixture_sha256,
      evaluator_script_sha256: bundle.manifest.evaluator_script_sha256,
      browser_manifest_sha256: validated.browser_manifest_sha256,
    },
    counts: {
      participants: 10,
      occasional: 5,
      regular: 5,
      sequences: 10,
      trials: 50,
      outbound: 0,
    },
    overall: {
      completion_rate: rate(completed, 50),
      unassisted_rate: rate(unassisted, 50),
      help_total: bundle.trials.reduce((sum, trial) => sum + trial.help_count, 0),
      critical_error_total: bundle.trials.filter((trial) => trial.critical_error !== 'none').length,
      median_seq: median(bundle.trials.map((trial) => trial.seq)),
    },
    tasks: taskMetrics,
  };
  if (bundleSha256 !== undefined) {
    if (!HEX_64.test(bundleSha256)) fail('bundle_sha256');
    result.binding.bundle_sha256 = bundleSha256;
  }
  return result;
}

function readEvidence(inputPath) {
  let stat;
  try {
    stat = fs.lstatSync(inputPath);
  } catch {
    fail('input.access');
  }
  if (stat.isSymbolicLink()) fail('input.symlink');
  if (!stat.isFile() || stat.size <= 0 || stat.size > MAX_EVIDENCE_BYTES) fail('input.size');
  const raw = fs.readFileSync(inputPath);
  let bundle;
  try {
    bundle = JSON.parse(raw.toString('utf8'));
  } catch {
    fail('input.json');
  }
  return { bundle, bundleSha256: sha256(raw) };
}

function runCli(argv = process.argv.slice(2), io = console) {
  try {
    if (argv.length !== 1 || typeof argv[0] !== 'string' || argv[0].length === 0) fail('usage');
    const { bundle, bundleSha256 } = readEvidence(argv[0]);
    io.log(JSON.stringify(summarizeStudy(bundle, bundleSha256)));
    return 0;
  } catch (error) {
    const code = error instanceof UxStudyContractError ? error.code : 'internal';
    io.error(JSON.stringify({ schema_version: 1, valid: false, error_code: code }));
    return 1;
  }
}

if (require.main === module) process.exitCode = runCli();

module.exports = {
  CRITICAL_ERRORS,
  SEQUENCES,
  TASKS,
  TRIAL_KEYS,
  UxStudyContractError,
  browserManifestDigest,
  canonicalJson,
  runCli,
  summarizeStudy,
  validateStudy,
};
