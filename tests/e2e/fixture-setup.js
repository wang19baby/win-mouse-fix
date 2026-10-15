'use strict';
const { startApp, stopApp } = require('./fixture');

// Playwright invokes the returned teardown even when a test fails.
module.exports = async () => {
  await startApp();
  return stopApp;
};
