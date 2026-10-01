(function () {
  // The developer script, injected into the entry HTML of the owner's own
  // apps by the bot (handlers/apps.rs): console capture and reload always,
  // the floating console under App Developer mode. Self-contained: no
  // network loads, and every style lives in this element's shadow root
  // under one prefixed root, so it can't touch the app and the app can't
  // touch it.
  if (window.__neboDevtools) return;
  var CFG = __NEBO_DEVTOOLS_CONFIG__;
  // A served page works out its app and routes from its own address (the
  // public prefix, /t/<botID> through the tunnel, is only knowable here).
  // The desktop's app window (neboapp://<id>/) is told them.
  var m = location.pathname.match(/^(.*?)\/apps\/([^/]+)\/ui(?:\/|$)/);
  if (!m && !CFG.appId) return;
  window.__neboDevtools = true;

  var prefix = m ? m[1] : '';
  var appId = CFG.appId || decodeURIComponent(m[2]);
  var api = CFG.api || location.origin + prefix + '/api/v1/apps/' + encodeURIComponent(appId);
  var socketUrl =
    CFG.socket ||
    (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + prefix + '/ws/app/' + encodeURIComponent(appId);
  var devlogUrl = api + '/devlog';
  var KEEP = 300;
  var logs = [];
  var queue = [];
  var flushTimer = null;
  var unseen = 0;
  var busy = false;
  var ui = null;
  var origFetch = typeof window.fetch === 'function' ? window.fetch.bind(window) : null;

  function str(a) {
    if (typeof a === 'string') return a;
    if (a instanceof Error) return (a.name || 'Error') + ': ' + a.message + (a.stack ? '\n' + a.stack : '');
    try {
      var s = JSON.stringify(a);
      return s === undefined ? String(a) : s;
    } catch (e) {
      return String(a);
    }
  }

  function short(url) {
    url = String(url || '');
    return url.indexOf(location.origin) === 0 ? url.slice(location.origin.length) : url;
  }

  function push(level, message, source) {
    if (busy) return;
    busy = true;
    try {
      var e = { level: level, message: String(message).slice(0, 2000), source: source, time: Date.now() };
      logs.push(e);
      if (logs.length > KEEP) logs.shift();
      if (level === 'error') unseen++;
      queue.push(e);
      if (queue.length >= 50) flush();
      else if (!flushTimer) flushTimer = setTimeout(flush, 400);
      if (ui) ui.changed();
    } finally {
      busy = false;
    }
  }

  // Batches go to the bot, which keeps the app's last entries for the
  // employee building it (app_console).
  function flush() {
    if (flushTimer) {
      clearTimeout(flushTimer);
      flushTimer = null;
    }
    if (!queue.length || !origFetch) return;
    var body = JSON.stringify({ entries: queue.splice(0, queue.length) });
    try {
      origFetch(devlogUrl, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: body,
        credentials: 'same-origin'
      }).catch(function () {});
    } catch (e) {}
  }

  window.addEventListener('pagehide', function () {
    if (!queue.length) return;
    var body = JSON.stringify({ entries: queue.splice(0, queue.length) });
    try {
      if (navigator.sendBeacon) navigator.sendBeacon(devlogUrl, new Blob([body], { type: 'application/json' }));
    } catch (e) {}
  });

  ['log', 'info', 'warn', 'error', 'debug'].forEach(function (k) {
    var orig = console[k];
    if (typeof orig !== 'function') return;
    console[k] = function () {
      try {
        orig.apply(console, arguments);
      } catch (e) {}
      push(k, Array.prototype.map.call(arguments, str).join(' '), 'console');
    };
  });

  window.addEventListener(
    'error',
    function (ev) {
      var t = ev.target;
      if (t && t !== window && t.nodeType === 1) {
        push('error', 'Failed to load ' + short(t.src || t.href || t.tagName), 'resource');
        return;
      }
      var where = ev.filename ? ' (' + short(ev.filename) + ':' + ev.lineno + ':' + ev.colno + ')' : '';
      var stack = ev.error && ev.error.stack ? '\n' + ev.error.stack : '';
      push('error', (ev.message || 'Error') + where + stack, 'error');
    },
    true
  );

  window.addEventListener('unhandledrejection', function (ev) {
    push('error', 'Unhandled promise rejection: ' + str(ev.reason), 'promise');
  });

  function netFail(method, url, status, text) {
    var what = status ? status + (text ? ' ' + text : '') : 'network error' + (text ? ': ' + text : '');
    push('error', method + ' ' + short(url) + ' → ' + what, 'network');
  }

  if (origFetch) {
    window.fetch = function (input, init) {
      var url = typeof input === 'string' ? input : (input && input.url) || String(input);
      var method = String((init && init.method) || (input && input.method) || 'GET').toUpperCase();
      var p = origFetch(input, init);
      if (String(url).indexOf(devlogUrl) === 0) return p;
      return p.then(
        function (r) {
          if (r.status >= 400) netFail(method, url, r.status, r.statusText);
          return r;
        },
        function (err) {
          if (!(err && err.name === 'AbortError')) netFail(method, url, 0, err && err.message ? err.message : String(err));
          throw err;
        }
      );
    };
  }

  if (window.XMLHttpRequest) {
    var xopen = XMLHttpRequest.prototype.open;
    var xsend = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function (method, url) {
      this.__nebo = { m: String(method || 'GET').toUpperCase(), u: String(url), aborted: false };
      return xopen.apply(this, arguments);
    };
    XMLHttpRequest.prototype.send = function () {
      var x = this;
      var d = x.__nebo;
      if (d) {
        x.addEventListener('abort', function () {
          d.aborted = true;
        });
        x.addEventListener('loadend', function () {
          if (d.aborted) return;
          if (x.status >= 400) netFail(d.m, d.u, x.status, x.statusText);
          else if (x.status === 0) netFail(d.m, d.u, 0, '');
        });
      }
      return xsend.apply(this, arguments);
    };
  }

  // app_reload: the employee asked every open view of this app to reload.
  // It arrives on the app's own event socket, the one the app SDK uses.
  function listen(delay) {
    var ws;
    try {
      ws = new WebSocket(socketUrl);
    } catch (e) {
      return;
    }
    ws.onopen = function () {
      delay = 1000;
    };
    ws.onmessage = function (ev) {
      var msg;
      try {
        msg = JSON.parse(ev.data);
      } catch (e) {
        return;
      }
      if (!msg || msg.type !== 'app_reload') return;
      var d = msg.data || {};
      var target = d.appId || d.agentId || d.app_id || d.agent_id;
      if (!target || target === appId) {
        flush();
        location.reload();
      }
    };
    ws.onclose = function () {
      setTimeout(function () {
        listen(Math.min(delay * 2, 30000));
      }, delay);
    };
  }
  listen(1000);

  // ── The floating console ────────────────────────────────────────────
  var CSS =
    '.ndt{font:13px/1.4 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;color:#e5e7eb;-webkit-text-size-adjust:100%}' +
    '.ndt *{box-sizing:border-box}' +
    '.ndt .ndt-fab{position:fixed;right:calc(12px + env(safe-area-inset-right));bottom:calc(12px + env(safe-area-inset-bottom));width:48px;height:48px;border-radius:24px;border:0;padding:0;background:#111827;color:#f9fafb;box-shadow:0 4px 14px rgba(0,0,0,.35);pointer-events:auto;touch-action:none;display:flex;align-items:center;justify-content:center;cursor:pointer;opacity:.92}' +
    '.ndt .ndt-fab:focus-visible,.ndt button:focus-visible{outline:2px solid #60a5fa;outline-offset:2px}' +
    '.ndt .ndt-fab svg{width:22px;height:22px}' +
    '.ndt .ndt-badge{position:absolute;top:-4px;right:-4px;min-width:20px;height:20px;padding:0 5px;border-radius:10px;background:#dc2626;color:#fff;font-size:11px;font-weight:700;line-height:20px;text-align:center}' +
    '.ndt [hidden]{display:none!important}' +
    '.ndt .ndt-sheet{position:fixed;left:0;right:0;bottom:0;margin:0 auto;max-width:720px;height:min(60vh,520px);background:#111827;border-radius:14px 14px 0 0;box-shadow:0 -8px 30px rgba(0,0,0,.4);pointer-events:auto;display:flex;flex-direction:column;padding:0 env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left)}' +
    '.ndt .ndt-bar{display:flex;align-items:center;gap:4px;padding:6px 8px;border-bottom:1px solid #1f2937;flex-wrap:wrap}' +
    '.ndt .ndt-tab,.ndt .ndt-btn{min-height:44px;min-width:44px;padding:0 12px;border:0;border-radius:8px;background:transparent;color:#d1d5db;font:inherit;font-weight:600;cursor:pointer}' +
    '.ndt .ndt-tab[aria-selected="true"]{background:#1f2937;color:#fff}' +
    '.ndt .ndt-btn{background:#1f2937}' +
    '.ndt .ndt-primary{background:#2563eb;color:#fff}' +
    '.ndt .ndt-btn:disabled{opacity:.5;cursor:default}' +
    '.ndt .ndt-spacer{flex:1}' +
    '.ndt .ndt-list{flex:1;overflow:auto;-webkit-overflow-scrolling:touch;font:12px/1.45 ui-monospace,SFMono-Regular,Menlo,monospace;margin:0;padding:0;list-style:none}' +
    '.ndt .ndt-row{display:flex;gap:8px;padding:6px 12px;border-bottom:1px solid #1f2937;white-space:pre-wrap;word-break:break-word}' +
    '.ndt .ndt-time{color:#6b7280;flex:none}' +
    '.ndt .ndt-msg{flex:1;min-width:0}' +
    '.ndt .ndt-error{color:#fca5a5;background:rgba(220,38,38,.1)}' +
    '.ndt .ndt-warn{color:#fcd34d;background:rgba(217,119,6,.1)}' +
    '.ndt .ndt-info{color:#93c5fd}' +
    '.ndt .ndt-debug{color:#9ca3af}' +
    '.ndt .ndt-empty{padding:24px 12px;color:#9ca3af;text-align:center;font-family:inherit}' +
    '.ndt .ndt-status{padding:4px 12px 8px;color:#9ca3af;font-size:12px;min-height:1em}';

  var ICON =
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="4 17 10 11 4 5"></polyline><line x1="12" y1="19" x2="20" y2="19"></line></svg>';

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function hhmmss(t) {
    var d = new Date(t);
    function p(n) {
      return (n < 10 ? '0' : '') + n;
    }
    return p(d.getHours()) + ':' + p(d.getMinutes()) + ':' + p(d.getSeconds());
  }

  function mount() {
    if (ui || !document.body) return;
    var host = document.createElement('nebo-devtools');
    // The host's own box: the one place an app's stylesheet could reach.
    host.setAttribute(
      'style',
      'all:initial!important;position:fixed!important;top:0!important;left:0!important;width:0!important;height:0!important;z-index:2147483647!important;pointer-events:none!important;'
    );
    var shadow = host.attachShadow ? host.attachShadow({ mode: 'open' }) : host;
    var style = document.createElement('style');
    style.textContent = CSS;
    var root = el('div', 'ndt');

    var fab = el('button', 'ndt-fab');
    fab.type = 'button';
    fab.setAttribute('aria-label', 'Open the app console');
    fab.innerHTML = ICON;
    var badge = el('span', 'ndt-badge');
    badge.hidden = true;
    fab.appendChild(badge);

    var sheet = el('section', 'ndt-sheet');
    sheet.setAttribute('role', 'dialog');
    sheet.setAttribute('aria-label', 'App console');
    sheet.hidden = true;

    var bar = el('div', 'ndt-bar');
    var tabConsole = el('button', 'ndt-tab', 'Console');
    var tabNetwork = el('button', 'ndt-tab', 'Network');
    [tabConsole, tabNetwork].forEach(function (t) {
      t.type = 'button';
      t.setAttribute('role', 'tab');
    });
    var spacer = el('span', 'ndt-spacer');
    var reload = el('button', 'ndt-btn', 'Reload');
    var send = el('button', 'ndt-btn ndt-primary', CFG.employee ? 'Send to ' + CFG.employee : 'Send errors');
    var clear = el('button', 'ndt-btn', 'Clear');
    var close = el('button', 'ndt-btn', '✕');
    close.setAttribute('aria-label', 'Close the app console');
    [reload, send, clear, close].forEach(function (b) {
      b.type = 'button';
    });
    bar.appendChild(tabConsole);
    bar.appendChild(tabNetwork);
    bar.appendChild(spacer);
    bar.appendChild(reload);
    bar.appendChild(send);
    bar.appendChild(clear);
    bar.appendChild(close);

    var list = el('ul', 'ndt-list');
    list.setAttribute('aria-live', 'polite');
    var status = el('div', 'ndt-status');
    sheet.appendChild(bar);
    sheet.appendChild(list);
    sheet.appendChild(status);

    root.appendChild(fab);
    root.appendChild(sheet);
    shadow.appendChild(style);
    shadow.appendChild(root);
    document.documentElement.appendChild(host);

    var tab = 'console';
    var pending = false;

    function render() {
      pending = false;
      var errors = logs.filter(function (e) {
        return e.level === 'error';
      }).length;
      var failed = logs.filter(function (e) {
        return e.source === 'network';
      });
      tabConsole.textContent = 'Console' + (logs.length ? ' (' + logs.length + ')' : '');
      tabNetwork.textContent = 'Network' + (failed.length ? ' (' + failed.length + ')' : '');
      tabConsole.setAttribute('aria-selected', String(tab === 'console'));
      tabNetwork.setAttribute('aria-selected', String(tab === 'network'));
      send.disabled = errors === 0;
      if (sheet.hidden) {
        badge.hidden = unseen === 0;
        badge.textContent = unseen > 99 ? '99+' : String(unseen);
        return;
      }
      unseen = 0;
      badge.hidden = true;
      var rows = tab === 'network' ? failed : logs;
      var atBottom = list.scrollTop + list.clientHeight >= list.scrollHeight - 8;
      list.textContent = '';
      if (!rows.length) {
        list.appendChild(el('li', 'ndt-empty', tab === 'network' ? 'No failed requests.' : 'Nothing logged yet.'));
        return;
      }
      rows.forEach(function (e) {
        var row = el('li', 'ndt-row ndt-' + e.level);
        row.appendChild(el('span', 'ndt-time', hhmmss(e.time)));
        row.appendChild(el('span', 'ndt-msg', e.message));
        list.appendChild(row);
      });
      if (atBottom) list.scrollTop = list.scrollHeight;
    }

    function changed() {
      if (pending) return;
      pending = true;
      if (window.requestAnimationFrame) window.requestAnimationFrame(render);
      else setTimeout(render, 16);
    }

    function open(on) {
      sheet.hidden = !on;
      fab.hidden = on;
      fab.setAttribute('aria-expanded', String(on));
      if (on) {
        status.textContent = '';
        render();
        list.scrollTop = list.scrollHeight;
        close.focus();
      } else {
        render();
        fab.focus();
      }
    }

    // Drag the button anywhere; a tap (no movement) opens the sheet.
    var drag = null;
    var dragged = false;
    fab.addEventListener('pointerdown', function (ev) {
      var r = fab.getBoundingClientRect();
      drag = { x: ev.clientX, y: ev.clientY, left: r.left, top: r.top, moved: false };
      try {
        fab.setPointerCapture(ev.pointerId);
      } catch (e) {}
    });
    fab.addEventListener('pointermove', function (ev) {
      if (!drag) return;
      var dx = ev.clientX - drag.x;
      var dy = ev.clientY - drag.y;
      if (!drag.moved && Math.abs(dx) + Math.abs(dy) < 6) return;
      drag.moved = true;
      var left = Math.max(4, Math.min(window.innerWidth - 52, drag.left + dx));
      var top = Math.max(4, Math.min(window.innerHeight - 52, drag.top + dy));
      fab.style.left = left + 'px';
      fab.style.top = top + 'px';
      fab.style.right = 'auto';
      fab.style.bottom = 'auto';
    });
    function endDrag() {
      if (drag && drag.moved) dragged = true;
      drag = null;
    }
    fab.addEventListener('pointerup', endDrag);
    fab.addEventListener('pointercancel', endDrag);
    fab.addEventListener('click', function () {
      if (dragged) {
        dragged = false;
        return;
      }
      open(true);
    });
    close.addEventListener('click', function () {
      open(false);
    });
    root.addEventListener('keydown', function (ev) {
      if (ev.key === 'Escape' && !sheet.hidden) open(false);
    });
    tabConsole.addEventListener('click', function () {
      tab = 'console';
      render();
    });
    tabNetwork.addEventListener('click', function () {
      tab = 'network';
      render();
    });
    clear.addEventListener('click', function () {
      logs.length = 0;
      unseen = 0;
      status.textContent = '';
      render();
    });
    reload.addEventListener('click', function () {
      flush();
      location.reload();
    });
    send.addEventListener('click', function () {
      flush();
      send.disabled = true;
      status.textContent = 'Sending…';
      // The bot sends the errors it holds for this app, so the batch above
      // goes first.
      setTimeout(function () {
        origFetch(api + '/devlog/send', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: '{}',
          credentials: 'same-origin'
        })
          .then(function (r) {
            status.textContent = r.ok
              ? 'Sent to ' + (CFG.employee || 'the employee') + '.'
              : r.status === 422
                ? 'No errors to send.'
                : 'Could not send. Try again.';
          })
          .catch(function () {
            status.textContent = 'Could not send. Try again.';
          })
          .then(render);
      }, 300);
    });

    ui = { changed: changed };
    render();
  }

  // The floating console is App Developer mode's; without it the page
  // still sends its console to the employee and reloads when asked.
  if (CFG.console === false) return;
  if (document.body) mount();
  else document.addEventListener('DOMContentLoaded', mount);
})();
