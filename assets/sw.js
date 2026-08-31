const CACHE_NAME = 'trackpad-v2';
const PRECACHE = [
  '/',
  '/manifest.json',
];

self.addEventListener('install', (e) => {
  e.waitUntil(
    caches.open(CACHE_NAME)
      .then(cache => cache.addAll(PRECACHE))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys().then(keys =>
      Promise.all(keys.filter(k => k !== CACHE_NAME).map(k => caches.delete(k)))
    ).then(() => self.clients.claim())
  );
});

self.addEventListener('fetch', (e) => {
  // Network-first for everything except WS/reports. Cached responses are
  // only used as an offline fallback, never preferred over the network.
  // This way a new build reaches phones immediately instead of serving
  // stale cached HTML/JS until the user manually clears site data.
  const url = new URL(e.request.url);
  if (url.pathname === '/ws' || url.pathname.startsWith('/report')) {
    return; // Let WebSocket + diagnostic requests pass through
  }
  e.respondWith(
    fetch(e.request).then(resp => {
      if (resp.ok) {
        const clone = resp.clone();
        caches.open(CACHE_NAME).then(cache => cache.put(e.request, clone));
      }
      return resp;
    }).catch(() => caches.match(e.request))
  );
});
