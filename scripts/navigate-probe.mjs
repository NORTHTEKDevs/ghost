// Is the background navigate reliable? Alternates a browser between two URLs
// several times and reports, per attempt, whether the title changed and whether
// the page ACTUALLY changed (read from the accessibility tree, not inferred).
//
//   node scripts/navigate-probe.mjs <ghost-mcp.exe> [rounds] [edge.exe]
//
// Runs on Ghost's hidden desktop, so nothing appears on the user's screen.
import { spawn } from 'node:child_process';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const exe = process.argv[2];
const rounds = Number(process.argv[3] ?? 4);
const edge = process.argv[4] || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const local = pathToFileURL(join(here, 'focus-testbed.html')).href;
const remote = 'https://example.com/';

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
server.stderr.on('data', d => { const s = d.toString(); if (/panick/i.test(s)) process.stderr.write(s); });
let nextId = 1;
const rpc = (method, params, timeoutMs = 90000) => new Promise((resolve, reject) => {
  const id = nextId++;
  const t = setTimeout(() => { pending.delete(id); reject(new Error(`timeout ${method}`)); }, timeoutMs);
  pending.set(id, m => { clearTimeout(t); resolve(m); });
  server.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
});
const call = async (name, args = {}) => {
  const m = await rpc('tools/call', { name, arguments: args });
  const text = m.result?.content?.[0]?.text ?? JSON.stringify(m.error ?? m);
  let b; try { b = JSON.parse(text); } catch { b = { raw: text }; }
  return { ok: b.ok !== false && !m.error, data: b.data ?? b, error: String(b.error ?? m.error?.message ?? '') };
};
const sleep = ms => new Promise(r => setTimeout(r, ms));

await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'navigate-probe', version: '0' } });
server.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');

const profile = `${process.env.TEMP}\\ghost-nav-probe-${Date.now()}`;
const launch = await call('ghost_window', { op: 'launch', exe: `"${edge}" --user-data-dir="${profile}" --no-first-run --no-default-browser-check --disable-sync --new-window ${local}` });
if (!launch.ok) { console.log('launch failed:', launch.error); process.exit(1); }
console.log(`launched on ${launch.data.target?.surface}: "${launch.data.target?.title}"`);
for (let i = 0; i < 40; i++) { const t = await call('ghost_see', { mode: 'text', limit: 4000 }); if (/First name/.test(t.data.text ?? '')) break; await sleep(500); }

let failures = 0;
for (let round = 0; round < rounds; round++) {
  for (const [label, url, expect] of [['-> example.com', remote, /Example Domain/], ['-> local testbed', local, /First name/]]) {
    const t0 = Date.now();
    const r = await call('ghost_wait', { for: 'navigate', url, timeout_ms: 15000 });
    const wall = Date.now() - t0;
    // What the page actually says now, independent of the title heuristic.
    let landed = false;
    for (let i = 0; i < 12 && !landed; i++) {
      const t = await call('ghost_see', { mode: 'text', limit: 4000 });
      landed = expect.test(t.data.text ?? '');
      if (!landed) await sleep(400);
    }
    const bad = !r.ok || !landed || r.data.arrived !== true;
    if (bad) failures++;
    console.log(`${bad ? 'FAIL' : 'PASS'} round ${round + 1} ${label.padEnd(18)} ${String(wall).padStart(6)}ms ok=${r.ok} arrived=${r.data.arrived} (title=${r.data.title_changed} url=${r.data.url_confirmed}) page_arrived=${landed}${r.error ? ' ERR ' + r.error.slice(0, 160) : ''}`);
  }
}
const name = (await call('ghost_window', { op: 'anchor' })).data.anchor?.title;
if (name) await call('ghost_window', { op: 'state', name, state: 'close' });
console.log(failures === 0 ? `ALL PASS (${rounds * 2} navigations)` : `${failures} of ${rounds * 2} navigations FAILED`);
server.stdin.end();
setTimeout(() => process.exit(failures === 0 ? 0 : 1), 400);
