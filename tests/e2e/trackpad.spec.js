const { test, expect } = require('@playwright/test');
const { server } = require('./fixture');

test('isolated app authenticates the real phone page', async ({ page }) => {
  const { url, token } = server();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(`${url}/?t=${token}`);
  await expect(page.locator('#pad')).toBeVisible();
  await expect(page.locator('#status')).toHaveText('已连接');
  expect(errors).toEqual([]);
});

test('invalid bearer cannot authenticate the phone page', async ({ page }) => {
  const { url, token } = server();
  const wrongToken = (token[0] === '0' ? '1' : '0') + token.slice(1);
  await page.goto(`${url}/?t=${wrongToken}`);
  await expect(page.locator('#banner')).toHaveClass(/show/);
  await expect(page.locator('#status')).toHaveText('认证失败');
});
