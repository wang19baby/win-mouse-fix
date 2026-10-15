'use strict';
const path = require('node:path');

module.exports = {
  testDir: './tests/e2e',
  timeout: 30000,
  retries: 0,
  workers: 1,
  globalSetup: path.join(__dirname, 'tests/e2e/fixture-setup.js'),
  use: {
    headless: true,
    viewport: { width: 390, height: 844 },
    serviceWorkers: 'allow',
  },
  reporter: [['list']],
};
