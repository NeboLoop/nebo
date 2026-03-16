// Finds the built CSS file and injects a <link> into 200.html
// This fixes FOUC (Flash of Unstyled Content) on SPA fallback routes
import { readFileSync, writeFileSync, readdirSync } from 'fs';
import { join } from 'path';

// Find CSS file in build output
const assetsDir = 'build/_app/immutable/assets';
const cssFiles = readdirSync(assetsDir).filter(f => f.endsWith('.css'));
if (cssFiles.length === 0) {
	console.error('No CSS files found in', assetsDir);
	process.exit(1);
}

const cssLink = `<link href="/_app/immutable/assets/${cssFiles[0]}" rel="stylesheet">`;

const fallbackHtml = readFileSync('build/200.html', 'utf-8');

// Inject into 200.html after the manifest link
const updatedFallback = fallbackHtml.replace(
	'<link rel="manifest" href="/site.webmanifest" />',
	`<link rel="manifest" href="/site.webmanifest" />\n\t${cssLink}`
);

writeFileSync('build/200.html', updatedFallback);
console.log('Injected CSS link into 200.html:', cssFiles[0]);
