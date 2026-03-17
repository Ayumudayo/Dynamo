module.exports = {
  apps: [
    {
      name: "dynamo-dashboard",
      cwd: __dirname,
      script: "./scripts/prod-dashboard.sh",
      interpreter: "bash",
      autorestart: true,
      watch: false,
      time: true,
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
      time: true,
      kill_timeout: 5000,
      env: {
        RUST_LOG:
          process.env.RUST_LOG ||
          "dynamo_bot=info,dynamo_app=info,dynamo_core=info,poise=info",
      },
    },
  ],
};

