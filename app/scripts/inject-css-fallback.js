// Extracts CSS link from index.html and injects into 200.html
// This fixes FOUC (Flash of Unstyled Content) on SPA fallback routes
import { readFileSync, writeFileSync } from 'fs';

const indexHtml = readFileSync('build/index.html', 'utf-8');
const fallbackHtml = readFileSync('build/200.html', 'utf-8');

// Extract CSS link from index.html
const cssLinkMatch = indexHtml.match(/<link href="[^"]*\.css" rel="stylesheet">/);
if (!cssLinkMatch) {
	console.error('No CSS link found in index.html');
	process.exit(1);
}

// Convert relative path (./_app) to absolute (/_app) for SPA routes
const absoluteCssLink = cssLinkMatch[0].replace('href="./_app', 'href="/_app');

// Inject into 200.html after the manifest link
const updatedFallback = fallbackHtml.replace(
	'<link rel="manifest" href="/site.webmanifest" />',
	`<link rel="manifest" href="/site.webmanifest" />\n\t${absoluteCssLink}`
);

writeFileSync('build/200.html', updatedFallback);
console.log('Injected CSS link into 200.html');
