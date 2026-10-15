// Only secure origins (HTTPS or localhost) can install this worker. Plain LAN
// HTTP remains an online-only trackpad; it does not weaken browser trust rules.
const PREFIX = 'win-mouse-fix-shell-';
const CACHE = `${PREFIX}v4`;
const SHELL = ['/', '/pad.html', '/winlist.html', '/manifest.json', '/drag.js'];
const SHELL_PATHS = new Set(SHELL);

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE).then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys().then((keys) => Promise.all(
      keys.filter((key) => key.startsWith(PREFIX) && key !== CACHE)
        .map((key) => caches.delete(key))
    )).then(() => self.clients.claim())
  );
});

self.addEventListener('fetch', (event) => {
  if (event.request.method !== 'GET') return;
  const url = new URL(event.request.url);
  // No dynamic/administrative route, token-bearing query string, window data,
  // thumbnail, report, diagnostic page, or unrelated origin enters Cache Storage.
  if (url.origin !== self.location.origin || !SHELL_PATHS.has(url.pathname)) return;

  event.respondWith((async () => {
    const key = new Request(url.origin + url.pathname);
    const cached = await caches.match(key);
    if (cached) return cached;
    try {
      return await fetch(event.request);
    } catch (error) {
      if (event.request.mode !== 'navigate') throw error;
      return new Response('离线：无法连接，请检查网络或服务器状态。', {
        status: 503,
        headers: { 'Content-Type': 'text/plain; charset=utf-8' },
      });
    }
  })());
});
