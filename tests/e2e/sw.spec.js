const { test, expect } = require('@playwright/test');
const { server } = require('./fixture');

test('browser installs a safe offline shell and shows disconnection', async ({ browser, request }) => {
  const { url, token } = server();
  const script = await request.get(`${url}/sw.js`);
  expect(script.status()).toBe(200);
  expect(script.headers()['content-type']).toMatch(/^application\/javascript/);

  const context = await browser.newContext({ serviceWorkers: 'allow' });
  try {
    const page = await context.newPage();
    await page.goto(`${url}/manifest.json`);
    await page.evaluate(() => caches.open('another-application-cache'));
    await page.goto(`${url}/?t=${token}`);
    await page.waitForFunction(() => navigator.serviceWorker.controller !== null);
    await expect(page.locator('#status')).toHaveText('已连接');
    expect(await page.evaluate(() => isSecureContext)).toBe(true);

    // Request the administrative QR page but never store or print its response.
    await page.evaluate(() => fetch('/qr').then((response) => response.status));
    const cached = await page.evaluate(async () => {
      const names = await caches.keys();
      const app = names.find((name) => name.startsWith('win-mouse-fix-shell-'));
      const keys = await (await caches.open(app)).keys();
      return { names, paths: keys.map((key) => new URL(key.url).pathname),
        queries: keys.map((key) => new URL(key.url).search) };
    });
    expect(cached.names).toContain('another-application-cache');
    expect(cached.paths.sort()).toEqual(['/', '/drag.js', '/manifest.json', '/pad.html', '/winlist.html']);
    expect(cached.queries).toEqual(['', '', '', '', '']);

    await context.setOffline(true);
    await page.reload({ waitUntil: 'domcontentloaded' });
    await expect(page.locator('#pad')).toBeVisible();
    await expect(page.locator('#banner')).toHaveClass(/show/);
    await expect(page.locator('#status')).not.toHaveText('已连接');
    await page.goto(`${url}/pad.html?t=${token}`);
    await expect(page.locator('#split')).toBeVisible();
    await page.goto(`${url}/winlist.html?t=${token}`);
    await expect(page.locator('#winListMode')).toBeAttached();
  } finally {
    await context.close();
  }
});
