#!/usr/bin/env node
// App Studio gate: greps an app folder for the patterns models fall into by
// habit and fails the build on any of them. Runs under bun or node with no
// dependencies.
//
//   node gate.js <app folder> [--others <folder holding the bot's other apps>]
//   execute(skill: "app-studio", script: "scripts/gate.js", args: { app, others })
//     (the execute tool passes args as JSON in SKILL_ARGS)
//
// Exit code 0 = pass (warnings may remain), 1 = at least one failure.

const fs = require('fs');
const path = require('path');

const args = process.argv.slice(2);
let skillArgs = {};
try { skillArgs = JSON.parse(process.env.SKILL_ARGS || '{}') || {}; } catch { skillArgs = {}; }
const appDir = path.resolve(skillArgs.app || args.find((a) => !a.startsWith('--')) || '.');
const othersIdx = args.indexOf('--others');
const othersDir = skillArgs.others ? path.resolve(skillArgs.others)
  : othersIdx >= 0 ? path.resolve(args[othersIdx + 1]) : path.dirname(appDir);

const fails = [];
const warns = [];
const fail = (msg) => fails.push(msg);
const warn = (msg) => warns.push(msg);
const rel = (p) => path.relative(appDir, p) || '.';

const MB = 1024 * 1024;
const MAX_FILE = 100 * MB;
const MAX_TOTAL = 500 * MB;
const AXES = ['palette', 'type', 'hero_layout', 'signature_effect', 'button_style', 'corner_shape'];
const ALLOWED_EXT = new Set([
  'html', 'css', 'js', 'mjs', 'jsx', 'tsx', 'ts', 'json', 'map', 'txt', 'md', 'csv', 'yaml', 'yml', 'toml',
  'png', 'jpg', 'jpeg', 'gif', 'svg', 'webp', 'avif', 'ico', 'bmp', 'pdf',
  'woff', 'woff2', 'ttf', 'otf',
  'mp3', 'wav', 'ogg', 'oga', 'm4a', 'aac', 'flac', 'opus',
  'mp4', 'webm', 'mov', 'm4v', 'wasm', 'glb', 'gltf',
]);
const TEXT_EXT = new Set(['html', 'htm', 'css', 'js', 'mjs', 'jsx', 'tsx', 'ts', 'svelte', 'vue', 'json']);
const BANNED_HEX = {
  'near-black + orange/amber': ['ff5c1a', 'ff6b35', 'e8590c', 'f97316', 'ea580c', 'd9480f', 'ff4b1f', 'f59e0b'],
  'near-black + neon cyan/blue/green': ['00e5ff', '22d3ee', '00ff88', '4ade80', '3b82f6', '00ffc2'],
  'purple/violet glow': ['8b5cf6', 'a855f7', '7c3aed', '6d28d9'],
  'beige + brass/clay/oxblood': ['f5f1ea', 'f7f5f1', 'fbf8f1', 'efeae0', 'ece6db', 'faf7f1', 'e8dfcb',
    'b08947', 'b6553a', '9a2436', '9c6e2a', 'bc7c3a', '7d5621', '1a1714', '1a1814', '1b1814'],
};
const BANNED_WORDS = /\b(elevate[sd]?|seamless(ly)?|unleash(es|ed)?|next-gen|revolutioni[sz]e[sd]?)\b/gi;
const PLACEHOLDERS = /lorem ipsum|REMOVE_THIS|\bTODO\b|\bFIXME\b|picsum\.photos|placeholder\.com|placehold\.co|dummyimage\.com|source\.unsplash\.com/gi;
const GENERIC = /modern and clean|sleek|minimal and elegant|blue accent|user-friendly|engaging experience|cutting-edge|vibrant/gi;

const ext = (p) => path.extname(p).slice(1).toLowerCase();
const read = (p) => { try { return fs.readFileSync(p, 'utf8'); } catch { return null; } };
const exists = (p) => fs.existsSync(p);

function walk(dir, out = []) {
  if (!exists(dir)) return out;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (e.name.startsWith('.') || e.name === 'node_modules') continue;
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, out);
    else if (e.isFile()) out.push(p);
  }
  return out;
}

// Simple front-matter reader: top-level `key: value` lines between --- fences.
function frontMatter(text) {
  const m = /^\uFEFF?---\r?\n([\s\S]*?)\r?\n---/.exec(text || '');
  if (!m) return null;
  const fm = { _raw: m[1] };
  for (const line of m[1].split(/\r?\n/)) {
    const kv = /^([A-Za-z_][\w-]*):\s*(.*)$/.exec(line);
    if (kv) fm[kv[1]] = kv[2].trim().replace(/^["']|["']$/g, '');
  }
  return fm;
}
const norm = (s) => String(s || '').toLowerCase().replace(/[^a-z0-9#]+/g, ' ').trim();

// Strip comments so code notes do not count as visible text.
function stripComments(text, e) {
  if (e === 'html' || e === 'htm' || e === 'svelte' || e === 'vue') text = text.replace(/<!--[\s\S]*?-->/g, '');
  if (e !== 'json') text = text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:\\'"`])\/\/.*$/gm, '$1');
  return text;
}

// ---------------------------------------------------------------- brief
const briefPath = path.join(appDir, 'brief.md');
const brief = read(briefPath);
let fm = null;
if (!brief) {
  fail('brief.md is missing from the app folder (Phase 1 comes before any code).');
} else {
  fm = frontMatter(brief);
  if (!fm) fail('brief.md has no front-matter block with the six axes.');
  else {
    for (const a of AXES) if (!fm[a]) fail(`brief.md front-matter is missing "${a}".`);
    if (!['generation', 'no-generation'].includes(fm.mode)) fail('brief.md front-matter "mode" must be generation or no-generation.');
    if (/^inter\b(?!\s*tight)/i.test(fm.type || '')) fail('brief.md: Inter is never the display face.');
  }
  const need = { 'Concept spine:': 1, 'Delivery tier:': 1, 'Palette:': 1, 'Type:': 1, 'Animation mode:': 1, '## Screens': 1, '## Assets': 1, '## CTA inventory': 1 };
  for (const k of Object.keys(need)) if (!brief.includes(k)) fail(`brief.md is missing "${k}".`);
  const pal = /^Palette:(.*)$/m.exec(brief);
  if (pal && (pal[1].match(/#[0-9a-f]{6}\b/gi) || []).length < 2) fail('brief.md Palette line needs exact hex values.');
  const tier = /^Delivery tier:\s*(\w+)/m.exec(brief);
  if (tier && !['cinema', 'spectacle', 'editorial'].includes(tier[1].toLowerCase())) fail('brief.md Delivery tier must be cinema, spectacle or editorial.');
  const anim = /^Animation mode:\s*(.*)$/m.exec(brief);
  if (anim && !/^(animated|still)\b/i.test(anim[1])) fail('brief.md Animation mode must be animated or still.');
  if (anim && /^still\s*$/i.test(anim[1])) fail('brief.md: "still" needs the owner\'s reason on the same line.');
  const body = brief.replace(/^---[\s\S]*?---/, '');
  const generic = [...new Set((body.match(GENERIC) || []).map((s) => s.toLowerCase()))];
  if (generic.length) fail(`brief.md has generic lines (${generic.join(', ')}); replace each with a decision.`);
  if (/<[A-Za-z][^<>\n]{1,40}>/.test(body)) fail('brief.md still has template <placeholders>.');
  if (/[\u2014\u2013]/.test(brief)) warn('brief.md has em or en dashes (keep them out of anything the owner reads).');
}

// --------------------------------------------- anti-convergence (4 of 6)
if (fm && exists(othersDir)) {
  for (const e of fs.readdirSync(othersDir, { withFileTypes: true })) {
    const other = path.join(othersDir, e.name);
    if (!e.isDirectory() || other === appDir) continue;
    const ofm = frontMatter(read(path.join(other, 'brief.md')));
    if (!ofm) continue;
    const same = AXES.filter((a) => fm[a] && norm(fm[a]) === norm(ofm[a]));
    if (same.length > 2) fail(`Too close to "${e.name}": same ${same.join(', ')}. Differ on at least 4 of 6 axes.`);
  }
}

// ---------------------------------------------------- AGENT.md, manifest
const agentMd = read(path.join(appDir, 'AGENT.md'));
if (!agentMd) fail('AGENT.md is missing.');
else {
  const afm = frontMatter(agentMd);
  if (!afm) fail('AGENT.md has no frontmatter.');
  else {
    if (afm.artifact_type !== 'app') fail('AGENT.md frontmatter needs "artifact_type: app" or the marketplace makes a plain employee.');
    for (const line of afm._raw.split(/\r?\n/)) {
      const kv = /^\s*([A-Za-z_][\w-]*):\s+(.+)$/.exec(line);
      if (kv && !/^["'|>[{]/.test(kv[2]) && kv[2].includes(': ')) {
        fail(`AGENT.md "${kv[1]}" contains ": " unquoted; quote the value or the app silently becomes a plain employee.`);
      }
    }
    if (/[\u2014\u2013]/.test(afm.description || '')) fail('AGENT.md description has an em or en dash.');
  }
}
const manifestText = read(path.join(appDir, 'manifest.json'));
if (!manifestText) fail('manifest.json is missing.');
else {
  try {
    const m = JSON.parse(manifestText);
    if (m.type !== 'app') fail('manifest.json needs "type": "app" (the key is "type", not "artifact_type").');
    if (/[\u2014\u2013]/.test(m.description || '')) fail('manifest.json description has an em or en dash.');
  } catch (err) {
    fail(`manifest.json is not valid JSON: ${err.message}`);
  }
}

// --------------------------------------------------------------- the page
const uiDir = path.join(appDir, 'ui');
const srcDir = path.join(appDir, 'src');
const uiFiles = walk(uiDir);
if (!exists(path.join(uiDir, 'index.html'))) fail('ui/index.html is missing.');
// Where files go: ui/ is served, the app folder is not.
if (exists(path.join(appDir, 'index.html'))) fail('index.html sits in the app folder itself; the only entry is ui/index.html (a build keeps its source entry in src/index.html).');
for (const p of ['package.json', 'node_modules', 'src', 'dist', 'vite.config.js', 'vite.config.mjs']) {
  if (exists(path.join(uiDir, p))) fail(`ui/${p} is inside the served folder; the build project lives in the app folder and writes only into ui/.`);
}
if (exists(path.join(uiDir, 'boards'))) fail('ui/boards/ is still there: design boards are working files; delete the folder before publishing.');

let total = 0;
for (const f of uiFiles) {
  const size = fs.statSync(f).size;
  total += size;
  if (size > MAX_FILE) fail(`${rel(f)} is ${(size / MB).toFixed(1)} MB; one file may be at most ${MAX_FILE / MB} MB.`);
  if (!ALLOWED_EXT.has(ext(f))) fail(`${rel(f)}: file type "${ext(f) || 'none'}" cannot be published.`);
}
if (total > MAX_TOTAL) fail(`ui/ is ${(total / MB).toFixed(1)} MB; an app may carry at most ${MAX_TOTAL / MB} MB.`);

// Authored text: everything in src/, plus ui/ files that are not build
// output (content-hashed), minified vendor code, or the SDK itself.
const built = (f) => /-[A-Za-z0-9_-]{8,}\.(m?js|css)$/.test(path.basename(f)) || /\.min\./.test(f) || path.basename(f) === 'nebo.global.js'
  || path.relative(uiDir, f).split(path.sep)[0] === 'bundle';
const authored = [...walk(srcDir), ...uiFiles.filter((f) => !built(f))].filter((f) => TEXT_EXT.has(ext(f)) && ext(f) !== 'json');
const texts = authored.map((f) => ({ f, e: ext(f), raw: read(f) || '' })).map((t) => ({ ...t, code: stripComments(t.raw, t.e) }));
const allCode = texts.map((t) => t.code).join('\n');
const uiText = uiFiles.filter((f) => TEXT_EXT.has(ext(f))).map((f) => read(f) || '').join('\n') + '\n' + allCode;
const override = fm && fm.brand_override;

for (const { f, code } of texts) {
  if (/[\u2014\u2013]|&mdash;|&ndash;|\\u201[34]/.test(code)) fail(`${rel(f)}: em or en dash in visible text; use a period, comma, colon or parentheses.`);
  const words = [...new Set((code.match(BANNED_WORDS) || []).map((w) => w.toLowerCase()))];
  if (words.length) fail(`${rel(f)}: banned words ${words.join(', ')}.`);
  const ph = [...new Set(code.match(PLACEHOLDERS) || [])];
  if (ph.length) fail(`${rel(f)}: placeholders ${ph.join(', ')}.`);
  if (/\bsrc=["']\s*["']/.test(code)) fail(`${rel(f)}: an empty src="".`);
  for (const [family, hexes] of Object.entries(BANNED_HEX)) {
    const hit = hexes.filter((h) => new RegExp(`#${h}\\b`, 'i').test(code));
    if (hit.length) (override ? warn : fail)(`${rel(f)}: banned palette family "${family}" (#${hit.join(', #')}).`);
  }
  if (/\bh-screen\b/.test(code)) fail(`${rel(f)}: h-screen; use min-h-dvh / 100dvh.`);
  if (/\b100vh\b/.test(code)) warn(`${rel(f)}: 100vh; prefer 100dvh (phone address bars).`);
  if (/grid-cols-3\b|repeat\(\s*3\s*,\s*(1fr|minmax)/.test(code)) warn(`${rel(f)}: a three-column grid; make sure it is not three equal cards in a row.`);
  if (/whileInView/.test(code) || (/(opacity:\s*0[;\s}]|opacity-0\b|autoAlpha:\s*0)/.test(code) && /(ScrollTrigger|IntersectionObserver)/.test(code))) {
    warn(`${rel(f)}: content may wait at opacity 0 for a scroll trigger; reveal with transforms instead.`);
  }
}

if (/font-family\s*:\s*["']?Inter["']?\s*[,;}]|family=Inter(?=[:&"'])/i.test(allCode)) {
  warn('Inter is loaded; it may be the body face but never the display face.');
}
const animates = /\bgsap\b|ScrollTrigger|@keyframes|\banimate\(|\bLenis\b|requestAnimationFrame|\.animate\(/.test(allCode);
if (animates && !/prefers-reduced-motion/.test(allCode)) fail('The app animates but nothing handles prefers-reduced-motion.');
if (!/touch-action/.test(allCode)) fail('No touch-action set: a scrolling page needs html, body { touch-action: pan-y; overscroll-behavior-x: none }.');
const index = read(path.join(uiDir, 'index.html')) || '';
if (index && !/name=["']viewport["']/i.test(index)) fail('ui/index.html has no viewport meta tag.');
if (/\bnebo\.(storage|identity|agents|janus|chat|surfaces)\b/.test(allCode) && !/NeboAppSDK/.test(uiText)) {
  fail('The code uses a bare "nebo" global; read it from window.NeboAppSDK.nebo.');
}
if (/NeboAppSDK/.test(allCode) && !/nebo\.global\.js/.test(index)) fail('ui/index.html does not load the SDK: <script src="../../../sdk/nebo.global.js"></script>.');
// Every path the page loads is relative: a leading "/" leaves ui/ on the phone.
for (const m of index.matchAll(/\b(?:src|href)\s*=\s*["']([^"']*)["']/gi)) {
  const u = m[1];
  if (u.startsWith('/') && !u.startsWith('//')) fail(`ui/index.html loads "${u}": a path starting with "/" breaks on the phone. Use ${u.includes('nebo.global.js') ? '"../../../sdk/nebo.global.js"' : `"./${u.replace(/^\/+/, '')}" (for Vite, base: './')`}.`);
}

const media = uiFiles.filter((f) => ['mp4', 'webm', 'mov', 'm4v'].includes(ext(f)));
if (media.length || /<video\b/i.test(allCode)) {
  if (!/playsinline/i.test(allCode)) fail('Video without playsinline: the phone opens its own full-screen player.');
  if (!/muted/i.test(allCode)) fail('Video without muted: phones refuse to start or prime it.');
}

// Every published file is used. A numbered frame sequence counts as used
// when its folder is named in the code.
const seqDirs = new Set();
for (const f of uiFiles) if (/^\d+\.(webp|png|jpe?g|avif)$/i.test(path.basename(f))) seqDirs.add(path.dirname(f));
for (const f of uiFiles) {
  const base = path.basename(f);
  if (base === 'index.html') continue;
  if (seqDirs.has(path.dirname(f)) && uiText.includes(path.basename(path.dirname(f)))) continue;
  if (!uiText.includes(base)) fail(`${rel(f)} is never referenced; use it or move it to refs/.`);
}

// The brief's promises.
if (fm && brief) {
  const imagery = uiFiles.filter((f) => ['png', 'jpg', 'jpeg', 'webp', 'avif', 'mp4', 'webm', 'glb', 'gltf'].includes(ext(f)));
  if (fm.mode === 'generation' && imagery.length === 0) fail('Mode is generation but ui/ holds no generated image, film or model.');
  if (fm.mode === 'no-generation' && !/<canvas|<svg|\.svg\b|WebGL|getContext\(/i.test(uiText)) {
    warn('Mode is no-generation but no canvas, SVG or shader art was found.');
  }
  if (/^A\d/i.test(fm.signature_effect || '') && media.length === 0 && seqDirs.size === 0) {
    fail('The signature effect is a film scrub but ui/ holds no film or frame sequence.');
  }
  if (/^Animation mode:\s*animated/im.test(brief) && !animates) fail('Animation mode is animated but nothing animates.');
}

// ---------------------------------------------------------------- report
for (const m of fails) console.log(`FAIL  ${m}`);
for (const m of warns) console.log(`WARN  ${m}`);
console.log(`\n${fails.length} failure(s), ${warns.length} warning(s) in ${appDir}`);
if (fails.length === 0) console.log('Gate passed. Review the warnings by eye, then look at it running (app_screenshot, app_console).');
process.exit(fails.length ? 1 : 0);
