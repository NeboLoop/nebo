# Motion Posts and Trailers

When the owner wants a video of the app (a motion post, a trailer):

1. Make a page for it in `ui/`, e.g. `render.html`. It plays the whole piece
   by itself from load, with no clicks, and reaches the app's data the way
   `index.html` does; a query (`render.html?key=launch`) picks what it shows.
   Time comes from the page's clock: CSS or Web animations,
   `requestAnimationFrame` timestamps, `performance.now()`, `<video>`. Never
   count frames.
2. `app_record(path: "render.html?key=launch", width: 1080, height: 1080, seconds: 10)`
   steps that clock one frame at a time (30 fps unless `fps` says otherwise,
   at most 60 seconds) and saves numbered PNG frames and a `manifest.json`
   into one folder. Square post 1080x1080, portrait 1080x1350, story or reel
   1080x1920, trailer 1920x1080. A page error fails the recording and keeps
   nothing: fix it and record again.
3. Encoding needs the Nebo Media plugin. Load its `video` skill
   (`use_skill("video")`) and run `video encode` with `frames` set to that
   folder: the mp4 lands beside it. If there is no `video` skill, Nebo Media
   is not installed: tell the owner a video needs it and ask whether to
   install it from the marketplace; until then, share the folder of frames.
   Never try ffmpeg or another encoder from the shell: it is not installed.
4. The mp4 reaches the owner as a card by itself (the plugin names it on
   its `Result:` line); don't share it again.
