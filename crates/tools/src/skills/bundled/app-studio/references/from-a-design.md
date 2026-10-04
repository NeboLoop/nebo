# From a design to an app

The owner says it in the chat, typed or spoken: "make this an app", "turn
this design into an app". You are the designing employee (Design Studio, or
any app that keeps designs) and you build the app here, in this chat. The
owner never leaves; the new app is its own employee, with its own window,
data and listing.

## What you start from

The design, from your own `app_data` (in Design Studio, the chat's
`chat:<chat>:design` key):

```
{ id, name, kind,
  theme: { colors: { primary, secondary, accent, surface, ink, muted },
           fonts: { display, body }, radius },
  screens: [{ id, name, html }] }
```

Each screen's `html` is body markup styled with Tailwind classes and the
theme's names (`bg-primary`, `font-display`, `rounded-theme`).

## Steps

1. **Load the tools** (Hard Rule 3), then create the app (Step 1, a new app
   employee), named after the design:
   - window: a website is `{ width: 1280, height: 860, resizable: true }`;
     app screens are `{ width: 390, height: 844, orientation: "portrait" }`.
   - `permissions: ["storage:readwrite"]`.
   - `agent_md`: what the app is for, in the owner's words, and: "Your page
     is built in <your name>. When the owner asks to change how the page
     looks or works, tell him to ask there."
   Write down the new app's name and id. Every app tool from here on takes
   `app: "<new app name>"`; never change your own page.
2. **Record the link** in your own store, so your canvas can show the app:
   `app_data set` with key `app:<design id>` and value `{ name, id }`.
3. **`ui/index.html`**: the SDK tag, the theme's font links, Tailwind and
   its config, `./style.css` and `./app.js`. Build the config and the font
   links exactly as the canvas does:

   ```html
   <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700;800&display=swap">
   <script src="https://cdn.tailwindcss.com"></script>
   <script>tailwind.config = { theme: { extend: {
     colors: { primary: "#2F5BEA", secondary: "#0F172A", accent: "#F59E0B", surface: "#FFFFFF", ink: "#0F172A", muted: "#64748B" },
     fontFamily: { display: ["Inter", "system-ui", "sans-serif"], body: ["Inter", "system-ui", "sans-serif"] },
     borderRadius: { theme: "12px" } } } };</script>
   ```

   The values are the design's theme. One Google Fonts link carries every
   family (`family=A:wght@...&family=B:wght@...`); Clash Display, Satoshi,
   General Sans, Cabinet Grotesk, Switzer, Sentient, Erode, Chillax,
   Panchang and Supreme load from Fontshare instead
   (`https://api.fontshare.com/v2/css?f[]=clash-display@400,500,600,700&display=swap`).
   `ui/style.css` sets the body font, `ink` color and `surface` background,
   and the display font on headings. The write saves Tailwind into the app.
4. **One screen per call**: `ui/screens/<name>.jsx`, the screen's markup as
   a component. Convert, don't redesign: `class` to `className`, `for` to
   `htmlFor`, void tags closed (`<img />`, `<br />`, `<input />`),
   `style="a: b"` to `style={{ a: 'b' }}`, `<!-- -->` to `{/* */}`, entities
   kept. Emoji become `lucide-react` icons (Hard Rule 14). Images the design
   uses (`assets/...`) are files in your own folder: copy each into the new
   app's `ui/assets/` and keep the same relative path.
5. **`ui/app.jsx`**: the shell. Navigation between the screens by their
   names, the first screen first. **`ui/store.js`**: the app's storage.
6. **Verify** (Step 4) against the new app: `app_reload`, `app_console` and
   `app_screenshot` with `app`. It must look like the design before
   anything else changes.
7. **Then behavior**, one thing at a time as the owner asks (forms that
   save, lists from storage, navigation), the same loop each time.

Tell the owner where to open it: "Your <name> app is ready. Open it from
your workforce."

## One writer

You built it, so you change it (Hard Rule 8) while the owner works with you.
The new app's own chat sends page changes back to you. Errors: read them
with `app_console(app:)`. The console's "Send to <app>" button in the new
app sends to the new app's own employee, not to you: if the owner used it,
pick the errors up with `app_console` and fix them here.
