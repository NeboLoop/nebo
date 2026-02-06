/**
 * Generate Nebo extension icons
 *
 * This script creates simple "N" icons in nebo's orange color.
 * Run with: node generate-icons.mjs
 *
 * Requires: npm install sharp
 */

import { writeFileSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'

const __dirname = dirname(fileURLToPath(import.meta.url))

// Nebo brand color (gold)
const NEBO_GOLD = '#ffbe18'

const sizes = [16, 32, 48, 128]

// Generate SVG for each size
function generateSVG(size) {
  const fontSize = Math.round(size * 0.65)
  const y = Math.round(size * 0.72)

  return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">
  <defs>
    <linearGradient id="bg" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" style="stop-color:${NEBO_GOLD}"/>
      <stop offset="100%" style="stop-color:#FF8F00"/>
    </linearGradient>
  </defs>
  <rect width="${size}" height="${size}" rx="${Math.round(size * 0.2)}" fill="url(#bg)"/>
  <text x="50%" y="${y}" text-anchor="middle" font-family="system-ui, -apple-system, sans-serif"
        font-size="${fontSize}" font-weight="700" fill="white">N</text>
</svg>`
}

// For browsers that support SVG icons, we just need the SVG files
// For PNG conversion, you'd need to use sharp or similar

for (const size of sizes) {
  const svg = generateSVG(size)
  const svgPath = join(__dirname, 'icons', `icon${size}.svg`)
  writeFileSync(svgPath, svg)
  console.log(`Generated: icons/icon${size}.svg`)
}

console.log(`
SVG icons generated! To convert to PNG:

Option 1: Use ImageMagick
  for size in 16 32 48 128; do
    convert icons/icon$size.svg icons/icon$size.png
  done

Option 2: Use sharp (install with: npm install sharp)
  Add sharp conversion code to this script

Option 3: Use online converter
  Upload SVGs to https://cloudconvert.com/svg-to-png
`)
