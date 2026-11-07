// pm2 app definition. Run `cargo build --release` first: pm2 runs the release binary.
module.exports = {
  apps: [
    {
      name: 'insta',
      script: 'target/release/insta',
      args: 'start',
      interpreter: 'none',
      cwd: __dirname,
      autorestart: true,
      max_restarts: 10,
      restart_delay: 5000,
      // A start that dies within 10s (port in use, bad config) counts against max_restarts.
      min_uptime: '10s',
    },
  ],
};
