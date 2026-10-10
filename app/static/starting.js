// The starting page's words over time. The app moves the window to Nebo as
// soon as the engine answers; until then it keeps trying, and this page
// only says how long it has been. `#held`: another copy of the app holds
// Nebo's port.
(function () {
  var opened = Date.now();
  var title = document.getElementById('title');
  var note = document.getElementById('note');
  var retry = document.getElementById('retry');
  retry.addEventListener('click', function () {
    location.reload();
  });
  function show() {
    var waited = Date.now() - opened;
    if (waited >= 120000) {
      title.textContent = 'Could not start Nebo. Try again.';
      note.hidden = true;
      retry.hidden = false;
    } else if (waited >= 30000) {
      note.textContent =
        location.hash === '#held'
          ? 'Nebo is already open in another copy of the app. Quit it to continue.'
          : 'Nebo is taking longer than usual to start.';
      note.hidden = false;
    }
  }
  setInterval(show, 1000);
  window.addEventListener('hashchange', show);
})();
