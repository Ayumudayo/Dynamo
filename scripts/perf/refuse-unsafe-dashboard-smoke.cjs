'use strict';

const fs = require('node:fs');

const MESSAGE =
  'REFUSED: dashboard smoke requires the isolated read-only runner; unsafe direct execution is disabled.';

fs.writeSync(process.stderr.fd, `${MESSAGE}\n`);
process.exit(2);
