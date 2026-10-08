// All build dependencies come from the temporary, locked npm installation.
import fs from 'node:fs/promises';
import path from 'node:path';
import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';

const root = path.resolve(process.argv[2]);
const require = createRequire(path.join(root, 'package.json'));
const { build } = require('esbuild');
// Pure JavaScript compression avoids differences between system and bundled zlib.
const { gzipSync } = require('fflate');
const output = path.join(root, 'output');
const modules = path.join(root, 'node_modules');
const manifest = JSON.parse(await fs.readFile(path.join(root, 'package.json'), 'utf8'));
await fs.mkdir(path.join(output, 'katex/fonts'), { recursive: true });

const result = await build({
  absWorkingDir: root,
  stdin: {
    contents: 'import mermaid from "mermaid/dist/mermaid.core.mjs"; window.mermaid = mermaid;',
    resolveDir: root,
    sourcefile: 'mermaid-entry.js',
  },
  bundle: true,
  format: 'iife',
  platform: 'browser',
  target: 'es2020',
  minify: true,
  legalComments: 'eof',
  metafile: true,
  write: false,
});
for (const item of Object.values(result.metafile.outputs)) {
  if (item.imports.length) throw new Error('The browser bundle must have no external imports');
}

// Record the actual input packages and their licences, including transitive code.
const packages = new Map();
for (const input of Object.keys(result.metafile.inputs)) {
  if (!input.startsWith('node_modules/')) continue;
  let dir = path.dirname(path.join(root, input));
  while (dir.startsWith(modules + path.sep)) {
    try {
      const pkg = JSON.parse(await fs.readFile(path.join(dir, 'package.json'), 'utf8'));
      if (pkg.name && pkg.version) {
        packages.set(`${pkg.name}@${pkg.version}`, { dir, pkg });
        break;
      }
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
    dir = path.dirname(dir);
  }
}
for (const name of ['mermaid', 'dompurify', 'katex']) {
  const found = [...packages.values()].filter(({ pkg }) => pkg.name === name);
  if (found.length !== 1 || found[0].pkg.version !== manifest.dependencies[name]) {
    throw new Error(`Expected exactly one pinned ${name} in the bundle`);
  }
}
let notices = 'Licences for packages included in the Mermaid browser bundle.\n';
for (const [key, { dir, pkg }] of [...packages].sort(([a], [b]) => a.localeCompare(b, 'en'))) {
  const files = (await fs.readdir(dir)).filter(name => /^(licen[cs]e|copying)(\.|$|-)/i.test(name)).sort();
  if (!files.length) throw new Error(`Missing licence text for ${key}`);
  notices += `\n${'='.repeat(72)}\n${key} (${pkg.license || 'see licence text'})\n`;
  for (const file of files) notices += `\n${await fs.readFile(path.join(dir, file), 'utf8')}\n`;
}
await fs.writeFile(path.join(output, 'LICENSE-mermaid-dependencies.txt'), notices.replace(/[ \t]+$/gm, '').trimEnd() + '\n');
await fs.copyFile(path.join(modules, 'mermaid/LICENSE'), path.join(output, 'LICENSE-mermaid.txt'));
await fs.copyFile(path.join(modules, 'katex/LICENSE'), path.join(output, 'katex/LICENSE.txt'));
await fs.writeFile(path.join(output, 'mermaid.min.js.gz'), gzipSync(result.outputFiles[0].contents, { level: 9, mtime: 0 }));
await fs.writeFile(path.join(output, 'katex/katex.min.js.gz'), gzipSync(await fs.readFile(path.join(modules, 'katex/dist/katex.min.js')), { level: 9, mtime: 0 }));
const css = (await fs.readFile(path.join(modules, 'katex/dist/katex.min.css'), 'utf8'))
  .replace(/,url\([^)]*\.woff\) format\("woff"\)/g, '')
  .replace(/,url\([^)]*\.ttf\) format\("truetype"\)/g, '');
await fs.writeFile(path.join(output, 'katex/katex.min.css'), css);
for (const name of await fs.readdir(path.join(modules, 'katex/dist/fonts'))) {
  if (name.endsWith('.woff2')) await fs.copyFile(path.join(modules, 'katex/dist/fonts', name), path.join(output, 'katex/fonts', name));
}
const sha256 = data => createHash('sha256').update(data).digest('hex');
await fs.writeFile(path.join(output, 'components.json'), JSON.stringify({
  mermaid: manifest.dependencies.mermaid,
  dompurify: manifest.dependencies.dompurify,
  katex: manifest.dependencies.katex,
  esbuild: manifest.dependencies.esbuild,
  fflate: manifest.dependencies.fflate,
  lockSHA256: sha256(await fs.readFile(path.join(root, 'package-lock.json'))),
  mermaidSHA256: sha256(await fs.readFile(path.join(output, 'mermaid.min.js.gz'))),
  katexSHA256: sha256(await fs.readFile(path.join(output, 'katex/katex.min.js.gz'))),
  bundledPackages: [...packages.keys()].sort(),
}, null, 2) + '\n');
