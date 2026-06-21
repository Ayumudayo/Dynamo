const fs = require("fs");
const path = require("path");

const logDir = path.join(__dirname, "logs");
fs.mkdirSync(logDir, { recursive: true });

module.exports = {
  apps: [
    {
      name: "dynamo-dashboard",
      cwd: __dirname,
      script: "./scripts/prod-dashboard.sh",
      interpreter: "bash",
      autorestart: true,
      watch: false,
      out_file: path.join(logDir, "dynamo-dashboard.out.log"),
      error_file: path.join(logDir, "dynamo-dashboard.error.log"),
      log_file: path.join(logDir, "dynamo-dashboard.combined.log"),
      time: false,
      kill_timeout: 5000,
      env: {
        RUST_LOG:
          process.env.RUST_LOG ||
          "dynamo_dashboard=info,dynamo_app=info,dynamo_core=info",
      },
    },
    {
      name: "dynamo-bot",
      cwd: __dirname,
      script: "./scripts/prod-bot.sh",
      interpreter: "bash",
      autorestart: true,
      watch: false,
      out_file: path.join(logDir, "dynamo-bot.out.log"),
      error_file: path.join(logDir, "dynamo-bot.error.log"),
      log_file: path.join(logDir, "dynamo-bot.combined.log"),
      time: false,
      kill_timeout: 5000,
      env: {
        RUST_LOG:
          process.env.RUST_LOG ||
          "dynamo_bot=info,dynamo_app=info,dynamo_core=info,poise=info",
      },
    },
  ],
};

