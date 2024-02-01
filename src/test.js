const moment = require('moment-timezone');
const timezoneChoices = moment.tz.names().map(tz => ({ name: tz, value: tz }));

console.debug(timezoneChoices);