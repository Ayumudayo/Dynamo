'use strict';

const fs = require('node:fs');

const {
  validateResultArtifact,
  safeErrorMessage,
} = require('./dashboard-load.cjs');
const { PerfContractError } = require('./assert-dashboard-instance.cjs');

const MAX_JSON_BYTES = 2 * 1024 * 1024;
const ALLOWED_BUDGET_KEYS = new Set([
  'max_p95_ms',
  'max_p95_ratio',
  'max_failed',
  'max_decoded_bytes_per_request',
  'require_wire_lt_decoded',
  'allowed_content_encodings',
]);
const ALLOWED_ENCODINGS = new Set(['identity', 'gzip', 'br']);

class BudgetAssertionError extends PerfContractError {
  constructor(name, decision) {
    super(`${name} budget exceeded`);
    this.name = 'BudgetAssertionError';
    this.decision = decision;
  }
}

function fail(message) {
  throw new PerfContractError(message);
}

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function assertNonnegativeNumber(value, label) {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    fail(`${label} must be a nonnegative finite number`);
  }
}

function assertNonnegativeInteger(value, label) {
  if (!Number.isSafeInteger(value) || value < 0) {
    fail(`${label} must be a nonnegative integer`);
  }
}

function validateBudget(value) {
  if (!isPlainObject(value)) fail('budget must be an object');
  const keys = Object.keys(value);
  if (keys.length === 0) fail('budget must contain at least one limit');
  for (const key of keys) {
    if (!ALLOWED_BUDGET_KEYS.has(key)) fail('unknown budget key detected');
  }
  if (!Object.prototype.hasOwnProperty.call(value, 'max_failed')) {
    fail('budget is missing max_failed');
  }
  if (keys.length === 1) fail('budget must contain a measurement limit');

  const result = { ...value };
  if (value.max_p95_ms !== undefined) assertNonnegativeNumber(value.max_p95_ms, 'max_p95_ms');
  if (value.max_p95_ratio !== undefined) {
    if (typeof value.max_p95_ratio !== 'number'
        || !Number.isFinite(value.max_p95_ratio)
        || value.max_p95_ratio <= 0) {
      fail('max_p95_ratio must be a positive finite number');
    }
  }
  assertNonnegativeInteger(value.max_failed, 'max_failed');
  if (value.max_decoded_bytes_per_request !== undefined) {
    assertNonnegativeInteger(value.max_decoded_bytes_per_request, 'max_decoded_bytes_per_request');
  }
  if (value.require_wire_lt_decoded !== undefined
      && value.require_wire_lt_decoded !== true) {
    fail('require_wire_lt_decoded may only be enabled with true');
  }
  if (value.allowed_content_encodings !== undefined) {
    if (!Array.isArray(value.allowed_content_encodings)
        || value.allowed_content_encodings.length === 0) {
      fail('allowed_content_encodings must be a nonempty array');
    }
    const unique = new Set();
    for (const encoding of value.allowed_content_encodings) {
      if (typeof encoding !== 'string' || !ALLOWED_ENCODINGS.has(encoding)) {
        fail('allowed_content_encodings contains an unsupported encoding');
      }
      if (unique.has(encoding)) fail('allowed_content_encodings contains a duplicate');
      unique.add(encoding);
    }
    result.allowed_content_encodings = [...value.allowed_content_encodings];
  }
  return result;
}

function assertCompatible(current, baseline) {
  const equal = current.schema_version === baseline.schema_version
    && current.runner_version === baseline.runner_version
    && current.fixture.version === baseline.fixture.version
    && current.fixture.sha256 === baseline.fixture.sha256
    && current.environment.fingerprint_sha256 === baseline.environment.fingerprint_sha256
    && current.environment.cargo_profile === baseline.environment.cargo_profile
    && current.instance.fixture_mode === baseline.instance.fixture_mode
    && current.path === baseline.path
    && current.requests === baseline.requests
    && current.concurrency === baseline.concurrency;
  if (!equal) fail('current and baseline artifacts are not compatible');
}

function assertBudgets({ current, budget, baseline = undefined }) {
  const validatedCurrent = validateResultArtifact(current, { retained: true });
  const validatedBudget = validateBudget(budget);
  let validatedBaseline;
  if (validatedBudget.max_p95_ratio !== undefined) {
    if (baseline === undefined) fail('baseline is required for max_p95_ratio');
    validatedBaseline = validateResultArtifact(baseline, { retained: true });
    assertCompatible(validatedCurrent, validatedBaseline);
    if (validatedBaseline.p95_ms <= 0) fail('baseline p95 must be positive for a ratio check');
  } else if (baseline !== undefined) {
    fail('baseline is only allowed for max_p95_ratio');
  }

  const checks = [];
  function check(name, observed, allowed, passed) {
    checks.push({ name, observed, allowed, passed });
  }

  if (validatedBudget.max_p95_ms !== undefined) {
    check(
      'p95_ms',
      validatedCurrent.p95_ms,
      validatedBudget.max_p95_ms,
      validatedCurrent.p95_ms <= validatedBudget.max_p95_ms,
    );
  }
  if (validatedBudget.max_p95_ratio !== undefined) {
    const ratio = validatedCurrent.p95_ms / validatedBaseline.p95_ms;
    check('p95 ratio', ratio, validatedBudget.max_p95_ratio, ratio <= validatedBudget.max_p95_ratio);
  }
  check(
    'failed requests',
    validatedCurrent.failed,
    validatedBudget.max_failed,
    validatedCurrent.failed <= validatedBudget.max_failed,
  );
  if (validatedBaseline !== undefined) {
    check(
      'baseline failed requests',
      validatedBaseline.failed,
      validatedBudget.max_failed,
      validatedBaseline.failed <= validatedBudget.max_failed,
    );
  }
  if (validatedBudget.max_decoded_bytes_per_request !== undefined) {
    const decodedPerRequest = validatedCurrent.decoded_bytes / validatedCurrent.requests;
    check(
      'decoded bytes per request',
      decodedPerRequest,
      validatedBudget.max_decoded_bytes_per_request,
      decodedPerRequest <= validatedBudget.max_decoded_bytes_per_request,
    );
  }
  if (validatedBudget.require_wire_lt_decoded === true) {
    check(
      'wire bytes below decoded bytes',
      validatedCurrent.wire_bytes,
      validatedCurrent.decoded_bytes,
      validatedCurrent.wire_bytes < validatedCurrent.decoded_bytes,
    );
  }
  if (validatedBudget.allowed_content_encodings !== undefined) {
    const observed = Object.keys(validatedCurrent.content_encodings).sort();
    const allowed = [...validatedBudget.allowed_content_encodings].sort();
    const allowedSet = new Set(allowed);
    check(
      'content encodings',
      observed.join(','),
      allowed.join(','),
      observed.every((encoding) => allowedSet.has(encoding)),
    );
  }

  const failed = checks.find((entry) => entry.passed === false);
  if (failed) {
    throw new BudgetAssertionError(
      failed.name,
      { schema_version: 1, passed: false, checks },
    );
  }
  return { schema_version: 1, passed: true, checks };
}

function readJsonFile(filePath, label) {
  if (typeof filePath !== 'string' || filePath.length === 0 || filePath.length > 32_767) {
    fail(`${label} path is required`);
  }
  let metadata;
  let body;
  try {
    metadata = fs.lstatSync(filePath);
    if (!metadata.isFile() || metadata.isSymbolicLink()) fail(`${label} must be a regular file`);
    if (metadata.size <= 0 || metadata.size > MAX_JSON_BYTES) fail(`${label} size is invalid`);
    body = fs.readFileSync(filePath, 'utf8');
  } catch (error) {
    if (error instanceof PerfContractError) throw error;
    fail(`${label} could not be opened`);
  }
  try {
    return JSON.parse(body);
  } catch (_error) {
    fail(`${label} is not valid JSON`);
  }
}

function parseArguments(argv) {
  if (!Array.isArray(argv) || (argv.length !== 2 && argv.length !== 4)) {
    fail('usage requires current result, budget, and optional --baseline result');
  }
  const parsed = { currentPath: argv[0], budgetPath: argv[1] };
  if (argv.length === 4) {
    if (argv[2] !== '--baseline') fail('unknown command-line argument');
    parsed.baselinePath = argv[3];
  }
  return parsed;
}

function runCli(argv = process.argv.slice(2), stdout = (value) => process.stdout.write(value)) {
  const args = parseArguments(argv);
  const current = readJsonFile(args.currentPath, 'current result');
  const budget = readJsonFile(args.budgetPath, 'budget');
  const baseline = args.baselinePath === undefined
    ? undefined
    : readJsonFile(args.baselinePath, 'baseline result');
  try {
    const decision = assertBudgets({ current, budget, baseline });
    stdout(`${JSON.stringify(decision)}\n`);
    return { decision, exitCode: 0 };
  } catch (error) {
    if (!(error instanceof BudgetAssertionError)) throw error;
    stdout(`${JSON.stringify(error.decision)}\n`);
    return { decision: error.decision, exitCode: 2 };
  }
}

if (require.main === module) {
  try {
    const execution = runCli();
    process.exitCode = execution.exitCode;
  } catch (error) {
    process.stderr.write(`assert-budgets failed: ${safeErrorMessage(error)}\n`);
    process.exitCode = 2;
  }
}

module.exports = {
  BudgetAssertionError,
  assertBudgets,
  assertCompatible,
  parseArguments,
  readJsonFile,
  runCli,
  validateBudget,
};
