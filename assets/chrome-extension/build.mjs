/**
 * Build script - bundles TypeScript and copies assets to dist/
 */

import * as esbuild from 'esbuild'
import { copyFileSync, mkdirSync, existsSync, readdirSync, rmSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const dist = join(__dirname, 'dist')
const watch = process.argv.includes('--watch')

// Clean dist
if (existsSync(dist)) {
  rmSync(dist, { recursive: true })
}
mkdirSync(dist, { recursive: true })

// Bundle TypeScript
const buildOptions = {
  entryPoints: [
    join(__dirname, 'src/background.ts'),
    join(__dirname, 'src/options.ts'),
  ],
  bundle: true,
  outdir: dist,
  format: 'esm',
  platform: 'browser',
  target: 'chrome120',
  minify: !watch,
  sourcemap: false,
}

if (watch) {
  const ctx = await esbuild.context(buildOptions)
  await ctx.watch()
  console.log('Watching for changes...')
} else {
  await esbuild.build(buildOptions)
}

// Copy static assets
copyFileSync(join(__dirname, 'manifest.json'), join(dist, 'manifest.json'))
copyFileSync(join(__dirname, 'options.html'), join(dist, 'options.html'))

// Copy icons
const iconsDir = join(__dirname, 'icons')
const distIcons = join(dist, 'icons')
mkdirSync(distIcons, { recursive: true })

for (const file of readdirSync(iconsDir)) {
  if (file.endsWith('.png')) {
    copyFileSync(join(iconsDir, file), join(distIcons, file))
  }
}

if (!watch) {
  console.log('Build complete: dist/')
}
