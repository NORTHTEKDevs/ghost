// Live timing probe for the 0.21.9 speed work, against a real ghost-mcp binary
// over stdio. Launches Edge with a throwaway profile on Ghost's hidden desktop
// (never on the user's screen), then measures:
//   1. ghost_wait for=navigate  (background address-bar navigate, returns on title change)
//   2. resolving the window by a STALE title after it retitled (title_drift path)
//   3. the old way for comparison: a fixed 6 s sleep, and a stale-title miss on a
//      title nobody has (the 2 s launch-race retry)
// Prints one line per measurement. Cleans up the hidden desktop at the end.
import { spawn } from 'node:child_process';

const exe = process.argv[2];
const server = spawn(exe, [], { stdio: ['pipe', 'pipe', 'pipe'] });
let buf = '';
const pending = new Map();
server.stdout.on('data', d => {
  buf += d.toString();
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i); buf = buf.slice(i + 1);
    if (!line.trim()) continue;
    try { const m = JSON.parse(line); if (m.id != null && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } } catch { process.stderr.write(`ghost-mcp stdout line is not JSON-RPC: ${line}\n`); }
  }
});
server.stderr.on('data', d => { const s = d.toString(); if (/panick|error/i.test(s)) process.stderr.write(s); });
let nextId = 1;
function rpc(method, params, timeoutMs = 30000) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => { pending.delete(id); reject(new Error(`timeout ${method}`)); }, timeoutMs);
    pending.set(id, m => { clearTimeout(t); resolve(m); });
    server.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
}
const call = async (name, args) => {
  const t0 = Date.now();
  const m = await rpc('tools/call', { name, arguments: args });
  const text = m.result?.content?.[0]?.text ?? JSON.stringify(m.error ?? m);
  let body; try { body = JSON.parse(text); } catch { body = { raw: text }; }
  // Ghost wraps tool payloads: { data: {...}, ok, ms, foreground }.
  const data = body.data ?? body;
  return { wall: Date.now() - t0, body, data };
};
const show = (label, r, pick = d => '') => {
  const err = r.body.ok === false ? ` ERROR: ${String(r.body.error ?? '').slice(0, 160)}` : '';
  console.log(`${label.padEnd(58)} wall=${String(r.wall).padStart(5)}ms server=${String(r.body.ms ?? '-').padStart(5)}ms ok=${r.body.ok ?? '?'} ${pick(r.data)}${err}`);
};

const sleep = ms => new Promise(r => setTimeout(r, ms));

await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'nav-probe', version: '0' } });
server.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');

const profile = process.env.TEMP + '\\ghost-nav-probe-profile';
const edge = process.env.EDGE_EXE || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const launch = await call('ghost_window', { op: 'launch', exe: `"${edge}" --user-data-dir="${profile}" --no-first-run --new-window https://example.com/` });
show('launch Edge (temp profile) on the hidden desktop', launch, b => `surface=${b.target?.surface} title="${b.target?.title}"`);
if (!launch.body.ok) { console.log(JSON.stringify(launch.body).slice(0, 400)); process.exit(1); }

// let the first page settle so the title is the real one
for (let i = 0; i < 40; i++) {
  const st = await call('ghost_window', { op: 'anchor' });
  if (/Example Domain/i.test(st.data.anchor?.title ?? st.data.target?.title ?? '')) break;
  await sleep(250);
}
const first = await call('ghost_see', { mode: 'text', limit: 200 });
const titleA = first.data.target?.title;
show('first page ready (anchor)', first, b => `title="${b.target?.title}"`);

// 1. background navigate, returns on title change
const nav = await call('ghost_wait', { for: 'navigate', url: 'https://www.iana.org/help/example-domains', timeout_ms: 15000 });
show('ghost_wait for=navigate (new path)', nav, b => `method=${b.method} changed=${b.title_changed} "${b.title_before}" -> "${b.title_after}"`);

// 2. stale title: ask for the window by the title it USED to have
const drift = await call('ghost_see', { window: titleA, mode: 'text', limit: 100 });
show('ghost_see by the OLD title (drift path)', drift, b => `source=${b.target?.source} drift=${JSON.stringify(b.target?.title_drift ?? null)}`);

// 3. a title nobody has: the launch-race retry still applies (control)
const miss = await call('ghost_see', { window: 'No Such Window Anywhere 4242', mode: 'text', limit: 50 });
show('ghost_see by a title nobody has (control: 2 s retry)', miss, b => (b.error ? 'error as expected' : 'UNEXPECTED hit'));

// 4. navigate again, no window given (anchor), to a page with a different title
const nav2 = await call('ghost_wait', { for: 'navigate', url: 'https://example.com/', timeout_ms: 15000, settle_ms: 0 });
show('ghost_wait for=navigate back, settle_ms=0', nav2, b => `changed=${b.title_changed} "${b.title_after}"`);

const close = await call('ghost_desktop_close', { id: 'auto' });
show('hidden desktop closed', close);
server.stdin.end();
setTimeout(() => process.exit(0), 500);
