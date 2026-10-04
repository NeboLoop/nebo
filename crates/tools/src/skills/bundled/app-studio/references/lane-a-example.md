# A Lane A app, worked

An index.html, an app.tsx with React and storage, and a Three.js scene module, as Step 3A sends them.

`ui/index.html` (plain HTML, written as is):

```html
<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>Orbit</title>
  <link rel="stylesheet" href="./style.css">
</head>
<body>
  <div id="root"></div>
  <script src="../../../sdk/nebo.global.js"></script>
  <script type="module" src="./app.js"></script>
</body>
</html>
```

`ui/app.tsx`:

```tsx
import { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { startScene } from './scene';

const { nebo } = (window as any).NeboAppSDK;

function App() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [best, setBest] = useState<number>(0);
  useEffect(() => { nebo.storage.getItem('best').then((v: number | null) => setBest(v ?? 0)); }, []);
  useEffect(() => startScene(canvas.current!), []);
  return <main><canvas ref={canvas} /><p>Best: {best}</p></main>;
}
createRoot(document.getElementById('root')!).render(<App />);
console.log('ready');
```

`ui/scene.ts` starts with `import * as THREE from 'three@0.170.0';` and
exports `startScene(canvas): () => void` (the cleanup).
