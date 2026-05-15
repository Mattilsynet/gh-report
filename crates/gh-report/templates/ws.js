// WebSocket auto-reconnect client for gh-report dashboard.
//
// Connects to /ws and listens for page-update events from the server.
// When the current page is in the update's affected page list, the
// browser reloads to show fresh content. Reconnects automatically
// with exponential backoff + jitter on disconnection.
//
// Protocol:
//   Server → Client:
//     {"type":"connected"}                         — handshake ack
//     {"type":"update","pages":[...],"repo":"..."}  — page cache updated
//     {"type":"reload"}                             — client lagged, full reload
//
//   Client → Server: (none — reserved for future admin commands)
(function () {
    'use strict';

    var RECONNECT_BASE_MS = 1000;
    var RECONNECT_MAX_MS = 30000;
    var attempt = 0;
    var ws;

    function currentPageKey() {
        // Map the browser path to the cache key used by the server.
        // "/" or "" → "index.html"; "/report.html" → "report.html"
        var p = location.pathname.replace(/^\//, '');
        return p || 'index.html';
    }

    function connect() {
        var proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        ws = new WebSocket(proto + '//' + location.host + '/ws');

        ws.onopen = function () {
            attempt = 0;
        };

        ws.onmessage = function (event) {
            var msg;
            try { msg = JSON.parse(event.data); } catch (_) { return; }

            if (msg.type === 'reload') {
                location.reload();
                return;
            }

            if (msg.type === 'update' && msg.pages) {
                var key = currentPageKey();
                for (var i = 0; i < msg.pages.length; i++) {
                    if (msg.pages[i] === key) {
                        location.reload();
                        return;
                    }
                }
            }
        };

        ws.onclose = function () {
            scheduleReconnect();
        };

        ws.onerror = function () {
            ws.close();
        };
    }

    function scheduleReconnect() {
        var delay = Math.min(
            RECONNECT_BASE_MS * Math.pow(1.5, attempt),
            RECONNECT_MAX_MS
        );
        // Add jitter (0–1000 ms) to prevent thundering herd on server restart.
        delay += Math.random() * 1000;
        attempt++;
        setTimeout(connect, delay);
    }

    // Skip WebSocket on warm-start pages — they have <meta http-equiv="refresh">
    // for polling until the first real collection completes. Once the first
    // full collection finishes and the cache is swapped (warm_start: false),
    // the next page load will not have the meta-refresh tag and will establish
    // the WebSocket connection. This is an intentional retention of the
    // warm_start mechanism as a meta-refresh polling fallback; full removal
    // of warm_start is deferred to a future cleanup.
    if (!document.querySelector('meta[http-equiv="refresh"]')) {
        connect();
    }
})();
