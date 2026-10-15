const { test, expect } = require('@playwright/test');
const { server } = require('./fixture');

// Real pages and real app authentication; only the OS-input WebSocket boundary
// is intercepted. Native button/selection proof lives in tools/drag_smoke.py.
async function phone(page, context, path, options = {}) {
  const { url, token } = server();
  const sent = [], sockets = [], errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.addInitScript(() => {
    document.addEventListener('pointerdown', (event) => { window.__pointerId = event.pointerId; }, true);
  });
  if (options.pauseFrames) {
    await page.addInitScript(() => {
      window.__frames = [];
      window.requestAnimationFrame = (callback) => { window.__frames.push(callback); return window.__frames.length; };
    });
  }
  await page.routeWebSocket('**/ws', (socket) => {
    sockets.push(socket);
    const peer = socket.connectToServer();
    peer.onMessage((message) => {
      const data = JSON.parse(message);
      // Window contents are unrelated to touch safety; don't display or record them.
      socket.send(data.t === 'window_list' ? JSON.stringify({ t: 'window_list', windows: [], desktops: [] }) : message);
    });
    socket.onMessage((message) => {
      const data = JSON.parse(message);
      if (['move', 'tap', 'gesture', 'drag', 'key', 'switch_window', 'switch_desktop'].includes(data.t)) {
        sent.push({ ...data, at: Date.now() });
        if (data.t === 'drag' && data.action === 'start' && !options.delayStart) {
          socket.send(JSON.stringify({ t: 'drag', id: data.id, state: options.rejectStart ? 'rejected' : 'started', reason: 'busy' }));
        }
        if (data.t === 'drag' && data.action === 'end' && !options.delayEnd) {
          socket.send(JSON.stringify({ t: 'drag', id: data.id, state: 'ended' }));
        }
      } else if (data.t === 'thumb_request') {
        // No screen capture needed for this client-lifecycle test.
      } else peer.send(message);
    });
  });
  await page.goto(`${url}${path}?t=${token}`);
  await expect(page.locator('#status')).toHaveText('已连接');
  const box = await page.locator('#pad').boundingBox();
  const point = { x: box.x + box.width / 2, y: box.y + box.height / 2, id: 1 };
  const cdp = await context.newCDPSession(page);
  const commands = (action) => sent.filter((message) => message.t === 'drag' && message.action === action);
  const touch = (type, points) => cdp.send('Input.dispatchTouchEvent', { type, touchPoints: points });
  const begin = async () => {
    const count = commands('start').length;
    await touch('touchStart', [point]);
    await expect.poll(() => commands('start').length).toBe(count + 1);
    return commands('start').at(-1).id;
  };
  const move = () => touch('touchMove', [{ ...point, x: point.x + 18 }]);
  const end = async (type = 'touchEnd') => {
    const count = commands('end').length;
    await touch(type, []);
    await expect.poll(() => commands('end').length).toBe(count + 1);
  };
  const flush = () => page.evaluate(() => window.__frames.splice(0).forEach((callback) => callback(performance.now())));
  const deliver = (id, state) => sockets.at(-1).send(JSON.stringify({ t: 'drag', id, state, reason: 'input_unavailable' }));
  return { sent, sockets, errors, cdp, point, commands, touch, begin, move, end, flush, deliver };
}

for (const path of ['/', '/pad.html']) {
  test.describe(`drag lifecycle ${path}`, () => {
    test('normal up flushes tagged motion before release and never clicks again', async ({ page, context }) => {
      const p = await phone(page, context, path, { pauseFrames: true });
      try {
        const id = await p.begin();
        await p.move();
        await p.end();
        await p.flush();
        const input = p.sent.filter((message) => message.t === 'move' || (message.t === 'drag' && message.action !== 'renew'));
        expect(input.map((message) => message.action || message.t)).toEqual(['start', 'move', 'end']);
        expect(input[1].drag_id).toBe(id);
        expect(input[2].id).toBe(id);
        expect(p.sent.filter((message) => message.t === 'tap')).toEqual([]);
        expect(p.errors).toEqual([]);
      } finally { await p.cdp.detach(); }
    });

    for (const abort of ['pointercancel', 'lostcapture', 'secondfinger', 'blur', 'pagehide', 'hidden']) {
      test(`${abort} discards motion, stops renewal and pairs release`, async ({ page, context }) => {
        const p = await phone(page, context, path, { pauseFrames: true });
        try {
          const id = await p.begin();
          await p.move();
          if (abort === 'pointercancel') await p.touch('touchCancel', []);
          else if (abort === 'lostcapture') {
            await page.evaluate(() => document.querySelector('#pad').releasePointerCapture(window.__pointerId));
            await p.touch('touchMove', [{ ...p.point, x: p.point.x + 19 }]);
          } else if (abort === 'secondfinger') {
            await p.touch('touchStart', [p.point, { ...p.point, id: 2, x: p.point.x + 35 }]);
          } else if (abort === 'hidden') {
            await page.evaluate(() => {
              Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
              document.dispatchEvent(new Event('visibilitychange'));
            });
          } else await page.evaluate((event) => window.dispatchEvent(new Event(event)), abort);
          await expect.poll(() => p.commands('end').length).toBe(1);
          expect(p.commands('end')[0].id).toBe(id);
          await p.flush();
          const renewals = p.commands('renew').length;
          await page.waitForTimeout(350);
          expect(p.commands('renew')).toHaveLength(renewals);
          if (abort !== 'pointercancel') await p.touch('touchEnd', []);
          expect(p.sent.filter((message) => message.t === 'move' || message.t === 'tap')).toEqual([]);
          expect(p.commands('end')).toHaveLength(1);
          expect(p.errors).toEqual([]);
        } finally { await p.cdp.detach(); }
      });
    }

    test('stationary hold continues heartbeat beyond the server lease', async ({ page, context }) => {
      const p = await phone(page, context, path);
      try {
        const id = await p.begin();
        await page.waitForTimeout(2600);
        const renewals = p.commands('renew');
        expect(renewals.every((message) => message.id === id)).toBe(true);
        const gaps = [p.commands('start')[0], ...renewals].slice(1).map((message, index) => message.at - [p.commands('start')[0], ...renewals][index].at);
        expect(Math.max(...gaps)).toBeLessThan(1500);
        expect(Date.now() - renewals.at(-1).at).toBeLessThan(1000);
        await p.end();
        expect(p.errors).toEqual([]);
      } finally { await p.cdp.detach(); }
    });

    test('cancelling before longpress does not press or click', async ({ page, context }) => {
      const p = await phone(page, context, path);
      try {
        await p.touch('touchStart', [p.point]);
        await page.evaluate(() => window.dispatchEvent(new Event('blur')));
        await page.waitForTimeout(550);
        await p.touch('touchEnd', []);
        expect(p.sent.filter((message) => ['drag', 'tap'].includes(message.t))).toEqual([]);
        expect(p.errors).toEqual([]);
      } finally { await p.cdp.detach(); }
    });

    test('late start acknowledgment cannot reactivate a cancelled gesture', async ({ page, context }) => {
      const p = await phone(page, context, path, { delayStart: true, delayEnd: true, pauseFrames: true });
      try {
        const id = await p.begin();
        await p.end('touchCancel');
        p.deliver(id, 'started');
        await p.touch('touchStart', [p.point]);
        await p.move();
        await p.flush();
        await page.waitForTimeout(350);
        expect(p.sent.filter((message) => message.t === 'move')).toEqual([]);
        expect(p.commands('renew').filter((message) => message.at > p.commands('end')[0].at)).toEqual([]);
        expect(p.commands('start')).toHaveLength(1);
        p.deliver(id, 'ended');
        await p.touch('touchEnd', []);
        const next = await p.begin();
        expect(next).toBeGreaterThan(id);
        await p.end();
        expect(p.errors).toEqual([]);
      } finally { await p.cdp.detach(); }
    });

    test('release failure blocks fresh gestures until an ended acknowledgment', async ({ page, context }) => {
      const p = await phone(page, context, path, { delayEnd: true });
      try {
        const id = await p.begin(); await p.end();
        p.deliver(id, 'release_failed');
        await page.waitForTimeout(50);
        await p.touch('touchStart', [p.point]); await page.waitForTimeout(550); await p.touch('touchEnd', []);
        expect(p.commands('start')).toHaveLength(1);
        p.deliver(id, 'ended');
        const next = await p.begin(); expect(next).toBeGreaterThan(id); await p.end();
        expect(p.errors).toEqual([]);
      } finally { await p.cdp.detach(); }
    });

    test('rejected start drops its touch and never creates a tap', async ({ page, context }) => {
      const p = await phone(page, context, path, { rejectStart: true });
      try {
        const id = await p.begin();
        await p.touch('touchEnd', []);
        const next = await p.begin(); expect(next).toBeGreaterThan(id);
        await p.touch('touchEnd', []);
        expect(p.sent.filter((message) => message.t === 'tap')).toEqual([]);
        expect(p.errors).toEqual([]);
      } finally { await p.cdp.detach(); }
    });

    test('transport close cancels the touch and reconnect uses a fresh id', async ({ page, context }) => {
      const p = await phone(page, context, path);
      try {
        const id = await p.begin(); const renewals = p.commands('renew').length;
        p.sockets.at(-1).close({ code: 1000, reason: 'test disconnect' });
        await page.waitForTimeout(400);
        expect(p.commands('renew')).toHaveLength(renewals);
        await expect(page.locator('#status')).toHaveText('已连接');
        await p.touch('touchEnd', []);
        expect(p.sent.filter((message) => message.t === 'tap')).toEqual([]);
        const next = await p.begin(); expect(next).toBeGreaterThan(id); await p.end();
        expect(p.errors).toEqual([]);
      } finally { await p.cdp.detach(); }
    });
  });
}
