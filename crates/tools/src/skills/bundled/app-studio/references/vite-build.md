# Step 3B: Vite build

Everything lives in the app folder from Check 1. Run each command with
`cd "<app folder>" &&` in front. These use npm because it is what the bot has.

```
<app folder>/
├── AGENT.md  manifest.json  agent.json   # the package (update_employee writes these)
├── package.json  vite.config.mjs          # build files, never served
├── node_modules/                          # never served, never published
├── src/
│   ├── index.html                         # the build's entry (not served)
│   └── main.tsx                           # or main.jsx
└── ui/                                    # build output + generated media: what Nebo serves
    ├── index.html                         # written by the build
    ├── bundle/                            # written by the build, replaced every build
    └── assets/                            # generate_media output, kept across builds
```

1. Set up once:

   ```bash
   npm init -y
   npm install --save-dev vite @vitejs/plugin-react
   npm install react react-dom three
   ```

2. Write `vite.config.mjs` exactly (write_file, in the app folder):

   ```js
   import { defineConfig } from 'vite';
   import react from '@vitejs/plugin-react';

   export default defineConfig({
     root: 'src',
     base: './',
     plugins: [react()],
     publicDir: false,
     build: { outDir: '../ui', emptyOutDir: false, assetsDir: 'bundle' },
   });
   ```

   - `base: './'` makes every built path relative (rule 7).
   - `outDir: '../ui'` is relative to `root`; it is the app's `ui/`.
   - `emptyOutDir: false` keeps `ui/assets/` (your generated media) alive;
     the build command below clears only `ui/bundle/`.

3. Write `src/index.html` with the SDK tag and the entry, both relative:

   ```html
   <!doctype html>
   <html>
   <head>
     <meta charset="utf-8">
     <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
     <title>Orbit</title>
   </head>
   <body>
     <div id="root"></div>
     <script src="../../../sdk/nebo.global.js"></script>
     <script type="module" src="./main.tsx"></script>
   </body>
   </html>
   ```

   Vite compiles TypeScript and JSX itself (no tsconfig needed), leaves
   the SDK tag alone (it is a classic script) and rewrites the module entry to `./bundle/index-<hash>.js`. Read the SDK at run time,
   `const { nebo } = window.NeboAppSDK;`; never install a package for it.
   Media from `generate_media` is referenced as `./assets/hero.webp`.

4. Build, every time:

   ```bash
   rm -rf ui/bundle && npx vite build
   ```

   Two warnings are expected and harmless: the SDK tag "can't be bundled
   without type=module", and chunks "larger than 500 kB".

5. **Check 3B.** `ui/index.html` names `./bundle/index-<hash>.js` and
   `../../../sdk/nebo.global.js`, and nothing that starts with `/`:

   ```bash
   grep -o '\(src\|href\)="[^"]*"' ui/index.html
   ```

Never hand-edit the built `ui/index.html` and never copy build output over
`src/`: change `src/`, rebuild. `crossorigin` and `type="module"` on the
built tags are correct; leave them.
