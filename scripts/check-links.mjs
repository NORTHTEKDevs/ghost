// Checks every link in the user-facing markdown: relative files and
// directories exist, #anchors match a heading in the target, and external URLs
// answer below 400. Links to this repository's own blob/tree URLs are checked
// against the working tree, so a page that is not pushed yet still resolves.
//
//   node scripts/check-links.mjs [--offline]
//
// Exit 0 when every link resolves, 1 otherwise. Fenced code is skipped.
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const offline = process.argv.includes('--offline');
const SELF = /^https:\/\/github\.com\/NORTHTEKDevs\/ghost\/(?:blob|tree)\/main\/([^#?]*)(#.*)?$/;

const pages = ['README.md', 'CONTRIBUTING.md', 'SECURITY.md', 'CODE_OF_CONDUCT.md', '.github/PULL_REQUEST_TEMPLATE.md'];
for (const f of readdirSync(join(root, 'docs'))) if (f.endsWith('.md')) pages.push(`docs/${f}`);
for (const d of ['plans', 'benches', 'audits', 'research']) pages.push(`docs/${d}/README.md`);

const slug = (heading) => heading.trim().toLowerCase().replace(/<[^>]+>/g, '').replace(/[^\p{L}\p{N}\s_-]/gu, '').replace(/\s/g, '-');
const anchorCache = new Map();
function anchors(file) {
  if (!anchorCache.has(file)) {
    const seen = new Map();
    const set = new Set();
    for (const line of readFileSync(file, 'utf8').replace(/^```[\s\S]*?^```/gm, '').split(/\r?\n/)) {
      const m = line.match(/^#{1,6}\s+(.*)$/);
      if (!m) continue;
      const base = slug(m[1]);
      const n = seen.get(base) ?? 0;
      seen.set(base, n + 1);
      set.add(n ? `${base}-${n}` : base);
    }
    anchorCache.set(file, set);
  }
  return anchorCache.get(file);
}

function stripCode(text) {
  return text.replace(/^```[\s\S]*?^```/gm, '').replace(/`[^`\n]*`/g, '');
}

function linksOf(text) {
  const out = [];
  const body = stripCode(text);
  for (const m of body.matchAll(/\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)/g)) out.push(m[1]);
  for (const m of body.matchAll(/(?:src|href)="([^"]+)"/g)) out.push(m[1]);
  return out;
}

function checkLocal(fromFile, target) {
  const [path, hash] = target.split('#');
  const file = path ? resolve(dirname(fromFile), decodeURIComponent(path)) : fromFile;
  if (!existsSync(file)) return `missing file ${relative(root, file)}`;
  if (hash && statSync(file).isFile() && file.endsWith('.md') && !anchors(file).has(decodeURIComponent(hash).toLowerCase())) {
    return `no heading for #${hash} in ${relative(root, file)}`;
  }
  return null;
}

async function checkRemote(url) {
  for (const method of ['HEAD', 'GET']) {
    try {
      const r = await fetch(url, { method, redirect: 'follow', signal: AbortSignal.timeout(20000), headers: { 'user-agent': 'ghost-link-check' } });
      if (r.status < 400) return null;
      if (method === 'GET') return `HTTP ${r.status}`;
    } catch (e) {
      if (method === 'GET') return e.cause?.code ?? e.name;
    }
  }
  return null;
}

const failures = [];
const remote = new Map();
let count = 0;
for (const page of pages) {
  const file = join(root, page);
  for (const link of linksOf(readFileSync(file, 'utf8'))) {
    count++;
    if (/^mailto:/.test(link)) continue;
    const self = link.match(SELF);
    if (self) {
      const err = checkLocal(join(root, 'README.md'), `${self[1]}${self[2] ?? ''}`);
      if (err) failures.push(`${page}: ${link} -> ${err}`);
    } else if (/^https?:/.test(link)) {
      if (!remote.has(link)) remote.set(link, []);
      remote.get(link).push(page);
    } else {
      const err = checkLocal(file, link);
      if (err) failures.push(`${page}: ${link} -> ${err}`);
    }
  }
}
if (!offline) {
  const results = await Promise.all([...remote.keys()].map(async (url) => [url, await checkRemote(url)]));
  for (const [url, err] of results) if (err) failures.push(`${remote.get(url)[0]}: ${url} -> ${err}`);
}

console.log(`${pages.length} pages, ${count} links, ${remote.size} distinct external URLs${offline ? ' (not fetched)' : ''}`);
for (const f of failures) console.log(`BROKEN ${f}`);
console.log(failures.length ? `${failures.length} broken` : 'all links resolve');
process.exit(failures.length ? 1 : 0);
