'use strict';
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const net = require('node:net');

let child;
let workDir;

function server() {
  const url = process.env.WMF_E2E_URL;
  const token = process.env.WMF_E2E_TOKEN;
  if (!url || !/^[a-f0-9]{64}$/.test(token || '')) {
    throw new Error('Run with npx playwright test: isolated app setup is missing');
  }
  return { url, token };
}

async function availablePort() {
  const listener = net.createServer();
  await new Promise((resolve, reject) => {
    listener.once('error', reject);
    listener.listen(0, '127.0.0.1', resolve);
  });
  const port = listener.address().port;
  await new Promise((resolve) => listener.close(resolve));
  return port;
}

function probe(port) {
  return new Promise((resolve) => {
    const req = http.get({ hostname: '127.0.0.1', port, path: '/manifest.json', timeout: 700 }, (response) => {
      const ready = response.statusCode === 200 &&
        (response.headers['content-type'] || '').startsWith('application/manifest+json');
      response.resume();
      resolve(ready);
    });
    req.once('error', () => resolve(false));
    req.once('timeout', () => req.destroy());
  });
}

async function startApp() {
  if (process.platform !== 'win32') throw new Error('Win32 desktop session required');
  const exe = path.resolve(process.env.WMF_E2E_EXE || 'target/debug/win-mouse-fix.exe');
  if (!fs.existsSync(exe)) throw new Error(`Build the app first: cargo build --offline --bin win-mouse-fix (${exe})`);
  workDir = fs.mkdtempSync(path.join(os.tmpdir(), 'win-mouse-fix-e2e-'));
  try {
    const port = await availablePort();
    const isolatedExe = path.join(workDir, 'win-mouse-fix.exe');
    fs.copyFileSync(exe, isolatedExe);
    fs.writeFileSync(path.join(workDir, 'config.toml'), `
[general]
start_hidden = true
[scroll]
enabled = false
smooth = false
speed = 1.0
invert = false
[buttons]
enabled = false
[remote]
enabled = true
local_only = true
port = ${port}
`);
    let stderr = '';
    child = spawn(isolatedExe, [], { cwd: workDir, windowsHide: true, stdio: ['ignore', 'ignore', 'pipe'] });
    child.stderr.on('data', (chunk) => { stderr = (stderr + chunk).slice(-1000); });
    const deadline = Date.now() + 20000;
    while (Date.now() < deadline) {
      if (child.exitCode !== null) throw new Error(`App exited early (${child.exitCode}): ${stderr}`);
      if (await probe(port)) {
        const tokenPath = path.join(workDir, 'token.txt');
        if (fs.existsSync(tokenPath)) {
          const match = fs.readFileSync(tokenPath, 'utf8').trim().match(/^v2:([a-f0-9]{64})$/);
          if (!match) throw new Error('Isolated app wrote an invalid token format');
          process.env.WMF_E2E_URL = `http://127.0.0.1:${port}`;
          process.env.WMF_E2E_TOKEN = match[1];
          return;
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 150));
    }
    throw new Error(`Isolated app did not become ready on loopback: ${stderr}`);
  } catch (error) {
    await stopApp();
    throw error;
  }
}

async function stopApp() {
  delete process.env.WMF_E2E_URL;
  delete process.env.WMF_E2E_TOKEN;
  if (child) {
    const owned = child;
    child = null;
    if (owned.exitCode === null) {
      const exited = new Promise((resolve) => owned.once('exit', resolve));
      owned.kill(); // Exact subprocess only; never touch another app instance.
      await exited;
    }
  }
  if (workDir) {
    fs.rmSync(workDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    workDir = null;
  }
}

// If the runner is interrupted before globalTeardown, do not leave an app behind.
process.once('exit', () => { if (child) child.kill(); });
module.exports = { startApp, stopApp, server };
