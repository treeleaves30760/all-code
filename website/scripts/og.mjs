// Regenerates static/img/og.png (1280x640) from the SVG template below.
//   cd website && npm run og
// Rasterised with @resvg/resvg-js (a devDependency) so the result does not
// depend on a browser or ImageMagick being installed. Fonts are resolved from
// the machine that runs it; the template names Helvetica Neue / Menlo with
// generic fallbacks, so any host produces a close rendering. Commit the PNG.
import {Resvg} from '@resvg/resvg-js';
import {writeFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {dirname, join} from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, '..', 'static', 'img', 'og.png');

const sans = 'Helvetica Neue, Inter, Arial, sans-serif';
const mono = 'Menlo, SF Mono, Consolas, monospace';
const agents = [
  ['Claude Code', 164, '#7c5cff'], ['Codex CLI', 136, '#7c5cff'],
  ['OpenCode', 140, '#38bdf8'], ['Pi', 66, '#38bdf8'], ['Copilot CLI', 146, '#38bdf8'],
  ['Goose', 98, '#4ade80'], ['Qwen Code', 142, '#4ade80'], ['Kimi Code', 146, '#4ade80'],
];
let x = 88;
const chips = agents.map(([label, w, stroke]) => {
  const g = `<g transform="translate(${x} 520)"><rect width="${w}" height="46" rx="23" fill="#1f2430" stroke="${stroke}" stroke-opacity="0.65" stroke-width="1.5"/><text x="${w / 2}" y="30" text-anchor="middle">${label}</text></g>`;
  x += w + 12;
  return g;
}).join('\n');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="640" viewBox="0 0 1280 640">
  <defs>
    <radialGradient id="glowA" cx="0.12" cy="0.1" r="0.6"><stop offset="0" stop-color="#7c5cff" stop-opacity="0.35"/><stop offset="1" stop-color="#7c5cff" stop-opacity="0"/></radialGradient>
    <radialGradient id="glowB" cx="0.95" cy="1.0" r="0.55"><stop offset="0" stop-color="#38bdf8" stop-opacity="0.22"/><stop offset="1" stop-color="#38bdf8" stop-opacity="0"/></radialGradient>
    <pattern id="grid" width="32" height="32" patternUnits="userSpaceOnUse"><path d="M32 0H0V32" fill="none" stroke="#ffffff" stroke-opacity="0.04"/></pattern>
  </defs>
  <rect width="1280" height="640" fill="#0f1117"/>
  <rect width="1280" height="640" fill="url(#grid)"/>
  <rect width="1280" height="640" fill="url(#glowA)"/>
  <rect width="1280" height="640" fill="url(#glowB)"/>
  <g transform="translate(88 84) scale(1.25)">
    <rect x="2" y="8" width="60" height="48" rx="10" fill="#1f2430" stroke="#7c5cff" stroke-width="3"/>
    <path d="M15 25l8 7-8 7" fill="none" stroke="#4ade80" stroke-width="4.5" stroke-linecap="round" stroke-linejoin="round"/>
    <path d="M29 41h16" fill="none" stroke="#7c5cff" stroke-width="4.5" stroke-linecap="round"/>
    <circle cx="45" cy="25" r="4.5" fill="#38bdf8"/>
  </g>
  <text x="192" y="152" font-family="${sans}" font-size="56" font-weight="700" fill="#f3f4f8" letter-spacing="-1.5">all-code</text>
  <text x="440" y="152" font-family="${mono}" font-size="44" fill="#a390ff">(alc)</text>
  <text x="1192" y="152" text-anchor="end" font-family="${mono}" font-size="20" fill="#5d6170">treeleaves30760.github.io/all-code</text>
  <text x="88" y="262" font-family="${sans}" font-size="46" font-weight="700" fill="#f3f4f8" letter-spacing="-1">Claude Code on your ChatGPT plan.</text>
  <text x="88" y="316" font-family="${sans}" font-size="27" fill="#9ea3b3">Eight coding agents · any provider · driven from your phone.</text>
  <rect x="88" y="366" width="600" height="112" rx="14" fill="#151821" stroke="#2a2f3c" stroke-width="2"/>
  <g font-family="${mono}" font-size="24">
    <text x="116" y="412" fill="#4ade80">$</text><text x="140" y="412" fill="#e6e7ec">codex login</text>
    <text x="116" y="452" fill="#4ade80">$</text><text x="140" y="452" fill="#e6e7ec">alc --codex claude</text>
  </g>
  <g font-family="${sans}" font-size="21" font-weight="600" fill="#e6e7ec">
${chips}
  </g>
</svg>`;

const png = new Resvg(svg, {fitTo: {mode: 'width', value: 1280}}).render().asPng();
writeFileSync(out, png);
console.log(`wrote ${out} (${png.length} bytes)`);
