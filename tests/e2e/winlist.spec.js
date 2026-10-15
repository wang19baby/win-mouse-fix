const { test, expect } = require('@playwright/test');
const { server } = require('./fixture');

test('window-list mode opens and returns without desktop input', async ({ page }) => {
  const { url, token } = server();
  await page.goto(`${url}/?t=${token}`);
  await expect(page.locator('#status')).toHaveText('已连接');
  await page.locator('#winListToggle').click();
  await expect(page).toHaveURL(/\/winlist\.html\?/);
  await expect(page.getByRole('dialog', { name: '窗口列表' })).toBeVisible();
  await page.locator('#winListBackBtn').click();
  await expect(page).toHaveURL(/\/\?t=/);
  await expect(page.locator('#pad')).toBeVisible();
});
