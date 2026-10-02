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
    // One row on every phone: never wraps, never scrolls. The two-segment
    // toggle on the left gives way first (its labels clip) on the narrowest
    // screen; Send and the icon buttons keep their size.
    '.ndt .ndt-bar{display:flex;flex-wrap:nowrap;align-items:center;gap:0;padding:6px;border-bottom:1px solid #1f2937;overflow:hidden}' +
    '.ndt .ndt-seg{display:flex;flex:0 1 auto;min-width:0;padding:2px;border-radius:10px;background:#0b1220}' +
    '.ndt .ndt-tab{position:relative;flex:0 1 auto;min-width:0;min-height:40px;padding:0 8px;border:0;border-radius:8px;background:transparent;color:#9ca3af;font:inherit;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;cursor:pointer}' +
    '.ndt .ndt-tab[aria-selected="true"]{background:#1f2937;color:#fff}' +
    // The count is a badge on the segment's corner, never more width.
    '.ndt .ndt-tab.ndt-has-count{padding-right:14px}' +
    '.ndt .ndt-tab .ndt-count{position:absolute;top:2px;right:1px;min-width:15px;height:15px;padding:0 4px;border-radius:8px;background:#dc2626;color:#fff;font-size:10px;font-weight:700;line-height:15px;text-align:center}' +
    '.ndt .ndt-btn{position:relative;flex:none;display:inline-flex;align-items:center;justify-content:center;gap:6px;min-height:44px;min-width:44px;padding:0;border:0;border-radius:8px;background:transparent;color:#d1d5db;font:inherit;font-weight:600;white-space:nowrap;cursor:pointer}' +
    '.ndt .ndt-btn svg{width:20px;height:20px;flex:none}' +
    '.ndt .ndt-primary{gap:5px;padding:0 11px 0 9px;margin:0 2px 0 6px;background:#2563eb;color:#fff}' +
    '.ndt .ndt-primary svg{width:16px;height:16px}' +
    '.ndt .ndt-btn:disabled{opacity:.5;cursor:default}' +
    '.ndt .ndt-lbl{display:none}' +
    '@media (min-width:560px){.ndt .ndt-lbl{display:inline}.ndt .ndt-icon{padding:0 10px}}' +
    '.ndt .ndt-btn,.ndt .ndt-tab{-webkit-touch-callout:none;-webkit-user-select:none;user-select:none}' +
    '.ndt .ndt-tip{position:absolute;top:-34px;right:12px;padding:4px 8px;border-radius:6px;background:#374151;color:#fff;font-size:12px;font-weight:600;white-space:nowrap;pointer-events:none}' +
    '.ndt .ndt-spacer{flex:1 1 0;min-width:0}' +
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

  // The bar's icons (stroke icons, the button's own colour).
  function svg(body) {
    return (
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
      body +
      '</svg>'
    );
  }
  var ICON_SEND = svg('<line x1="22" y1="2" x2="11" y2="13"></line><polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>');
  var ICON_RELOAD = svg('<polyline points="23 4 23 10 17 10"></polyline><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>');
  var ICON_CLEAR = svg('<polyline points="3 6 5 6 21 6"></polyline><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path><path d="M10 11v6"></path><path d="M14 11v6"></path><path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>');
  var ICON_CLOSE = svg('<line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line>');

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

    // One row: Console | Network on the left; Send, then reload, clear and
    // close as icons on the right (their words show beside them on a wide
    // screen, and are always their accessible names).
    var bar = el('div', 'ndt-bar');
    var seg = el('div', 'ndt-seg');
    seg.setAttribute('role', 'tablist');
    var tabConsole = el('button', 'ndt-tab');
    var tabNetwork = el('button', 'ndt-tab');
    var consoleCount = el('span', 'ndt-count');
    var networkCount = el('span', 'ndt-count');
    tabConsole.appendChild(document.createTextNode('Console'));
    tabConsole.appendChild(consoleCount);
    tabNetwork.appendChild(document.createTextNode('Network'));
    tabNetwork.appendChild(networkCount);
    [tabConsole, tabNetwork].forEach(function (t) {
      t.type = 'button';
      t.setAttribute('role', 'tab');
      seg.appendChild(t);
    });
    var spacer = el('span', 'ndt-spacer');
    function iconButton(cls, icon, label, name) {
      var b = el('button', 'ndt-btn ' + cls);
      b.type = 'button';
      b.innerHTML = icon;
      b.appendChild(el('span', cls === 'ndt-primary' ? null : 'ndt-lbl', label));
      b.setAttribute('aria-label', name);
      b.title = name;
      return b;
    }
    var sendName = CFG.employee ? 'Send to ' + CFG.employee : 'Send errors';
    var send = iconButton('ndt-primary', ICON_SEND, 'Send', sendName);
    var reload = iconButton('ndt-icon', ICON_RELOAD, 'Reload', 'Reload');
    var clear = iconButton('ndt-icon', ICON_CLEAR, 'Clear', 'Clear');
    var close = iconButton('ndt-icon', ICON_CLOSE, 'Close', 'Close');
    close.setAttribute('aria-label', 'Close the app console');
    // A long press on Send names who it goes to (a phone has no hover).
    var tip = el('span', 'ndt-tip', sendName);
    tip.hidden = true;
    var tipTimer = null;
    send.addEventListener('pointerdown', function () {
      clearTimeout(tipTimer);
      tipTimer = setTimeout(function () {
        tip.hidden = false;
      }, 450);
    });
    ['pointerup', 'pointercancel', 'pointerleave'].forEach(function (t) {
      send.addEventListener(t, function () {
        clearTimeout(tipTimer);
        setTimeout(function () {
          tip.hidden = true;
        }, t === 'pointerup' && !tip.hidden ? 1200 : 0);
      });
    });
    bar.appendChild(seg);
    bar.appendChild(spacer);
    bar.appendChild(send);
    bar.appendChild(reload);
    bar.appendChild(clear);
    bar.appendChild(close);

    var list = el('ul', 'ndt-list');
    list.setAttribute('aria-live', 'polite');
    var status = el('div', 'ndt-status');
    sheet.appendChild(bar);
    sheet.appendChild(list);
    sheet.appendChild(status);
    sheet.appendChild(tip);

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
      consoleCount.hidden = errors === 0;
      tabConsole.classList.toggle('ndt-has-count', errors > 0);
      tabNetwork.classList.toggle('ndt-has-count', failed.length > 0);
      consoleCount.textContent = errors > 99 ? '99+' : String(errors);
      tabConsole.setAttribute('aria-label', 'Console' + (errors ? ', ' + errors + (errors === 1 ? ' error' : ' errors') : ''));
      networkCount.hidden = failed.length === 0;
      networkCount.textContent = failed.length > 99 ? '99+' : String(failed.length);
      tabNetwork.setAttribute('aria-label', 'Network' + (failed.length ? ', ' + failed.length + ' failed' : ''));
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
      // The request carries the page's own errors too: a batch still on its
      // way (or lost) can never leave the employee with nothing. The bot
      // keeps each once, with what it already holds. Nothing here is
      // cleared, so a failed send is sent again by the same button.
      var errors = logs.filter(function (e) {
        return e.level === 'error';
      });
      var body = JSON.stringify({ entries: errors.slice(-50) });
      setTimeout(function () {
        origFetch(api + '/devlog/send', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: body,
          credentials: 'same-origin'
        })
          .then(function (r) {
            // "Sent" only on the bot's own word that the message is on its
            // way (`status: dispatched`); its refusal is shown as it says it.
            return r
              .json()
              .catch(function () {
                return {};
              })
              .then(function (b) {
                status.textContent =
                  r.ok && b && b.status === 'dispatched'
                    ? 'Sent to ' + (CFG.employee || 'the employee') + '. It is working on them in your chat.'
                    : (b && typeof b.error === 'string' && b.error) || 'Could not send. Try again.';
              });
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
