// The page's clock for `app_record`, installed before the page's first
// script. Time stands still until the recorder moves it: `__neboClock.to(ms)`
// runs the timers due by then, then the animation frame callbacks, and sets
// every CSS, Web and SVG animation and every video to that moment. Each frame
// is drawn whole at its own moment however slow the machine is.
(function () {
  if (window.__neboClock) return;
  var RealDate = Date;
  var realSetTimeout = window.setTimeout.bind(window);
  var realClearTimeout = window.clearTimeout.bind(window);
  var epoch = RealDate.now();
  var now = 0;
  var timers = new Map();
  var nextTimer = 1;
  var depth = 0;
  var frames = new Map();
  var nextFrame = 1;

  // A task of its own, never throttled: lets the page's own work (a promise
  // chain, a scheduler's message) run between steps.
  var channel = new MessageChannel();
  var waiting = [];
  channel.port1.onmessage = function () {
    var w = waiting.shift();
    if (w) w();
  };
  function task(fn) {
    waiting.push(fn);
    channel.port2.postMessage(0);
  }

  function call(fn, args) {
    try {
      if (typeof fn === 'function') fn.apply(window, args);
      else (0, eval)(String(fn));
    } catch (e) {
      // Uncaught, as it would have been: the page's error handlers see it.
      if (typeof window.reportError === 'function') window.reportError(e);
      else
        task(function () {
          throw e;
        });
    }
  }

  // Timers due by `limit`, earliest first, each run at its own moment.
  function runTimers(limit) {
    for (var n = 0; n < 10000; n++) {
      var id = 0;
      var best = null;
      timers.forEach(function (t, k) {
        if (t.at <= limit && (!best || t.at < best.at)) {
          best = t;
          id = k;
        }
      });
      if (!best) return;
      if (best.at > now) now = best.at;
      if (best.every) best.at += best.every;
      else timers.delete(id);
      depth++;
      call(best.fn, best.args);
      depth--;
    }
  }

  var pumping = false;
  function pump() {
    if (pumping) return;
    pumping = true;
    task(function () {
      pumping = false;
      runTimers(now);
    });
  }

  function addTimer(fn, ms, args, repeat) {
    var delay = Math.max(0, Number(ms) || 0);
    // A timer set from a timer waits at least 4 ms, as browsers make it.
    if (depth > 0 && delay < 4) delay = 4;
    if (repeat) delay = Math.max(1, delay);
    var id = nextTimer++;
    timers.set(id, { at: now + delay, every: repeat ? delay : 0, fn: fn, args: args });
    if (delay === 0) pump();
    return id;
  }
  window.setTimeout = function (fn, ms) {
    return addTimer(fn, ms, Array.prototype.slice.call(arguments, 2), false);
  };
  window.setInterval = function (fn, ms) {
    return addTimer(fn, ms, Array.prototype.slice.call(arguments, 2), true);
  };
  window.clearTimeout = window.clearInterval = function (id) {
    timers.delete(id);
  };
  window.requestAnimationFrame = function (cb) {
    var id = nextFrame++;
    frames.set(id, cb);
    return id;
  };
  window.cancelAnimationFrame = function (id) {
    frames.delete(id);
  };

  performance.now = function () {
    return now;
  };
  function FakeDate() {
    if (!new.target) return new RealDate(epoch + now).toString();
    var args = arguments.length ? arguments : [epoch + now];
    return Reflect.construct(RealDate, args, new.target);
  }
  FakeDate.prototype = RealDate.prototype;
  FakeDate.now = function () {
    return epoch + now;
  };
  FakeDate.parse = RealDate.parse;
  FakeDate.UTC = RealDate.UTC;
  window.Date = FakeDate;

  // Each animation plays from the moment it was first seen.
  var started = new WeakMap();
  function seekAnimations() {
    if (typeof document.getAnimations !== 'function') return;
    document.getAnimations().forEach(function (a) {
      if (!started.has(a)) {
        if (a.playState !== 'running') return;
        started.set(a, now);
        a.pause();
      }
      if (a.playState === 'finished') return;
      var t = (now - started.get(a)) * (a.playbackRate || 1);
      var end = a.effect && a.effect.getComputedTiming ? a.effect.getComputedTiming().endTime : Infinity;
      if (isFinite(end) && t >= end) a.finish();
      else a.currentTime = t;
    });
    var svgs = document.querySelectorAll('svg');
    for (var i = 0; i < svgs.length; i++) {
      var s = svgs[i];
      if (!s.ownerSVGElement && typeof s.setCurrentTime === 'function') {
        s.pauseAnimations();
        s.setCurrentTime(now / 1000);
      }
    }
  }

  // Videos are set to the frame's moment and the frame waits for the seek.
  var videoStart = new WeakMap();
  function seekVideos() {
    var waits = [];
    var videos = document.querySelectorAll('video');
    for (var i = 0; i < videos.length; i++) {
      var v = videos[i];
      if (!(v.duration > 0)) continue;
      if (!videoStart.has(v)) {
        if (v.paused && !v.autoplay) continue;
        videoStart.set(v, now - v.currentTime * 1000);
      }
      v.pause();
      var t = (now - videoStart.get(v)) / 1000;
      t = v.loop ? t % v.duration : Math.min(t, v.duration);
      if (Math.abs(v.currentTime - t) < 0.001) continue;
      waits.push(
        new Promise(function (done) {
          var video = v;
          var timer = realSetTimeout(done, 1000);
          video.addEventListener(
            'seeked',
            function () {
              realClearTimeout(timer);
              done();
            },
            { once: true }
          );
          video.currentTime = t;
        })
      );
    }
    return waits;
  }
  window.__neboClock = {
    // Move the page's clock to `ms` after it opened and draw that moment.
    to: function (ms) {
      runTimers(ms);
      now = Math.max(now, ms);
      var due = frames;
      frames = new Map();
      due.forEach(function (cb) {
        call(cb, [now]);
      });
      seekAnimations();
      return Promise.all(seekVideos()).then(function () {
        return new Promise(function (done) {
          task(function () {
            done(now);
          });
        });
      });
    }
  };
})();
