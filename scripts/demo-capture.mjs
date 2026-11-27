// Records the README demo: an agent's real calls against a Ghost-owned window.
//
//   node scripts/demo-capture.mjs <ghost-mcp.exe> <ghost-testbed.exe> <out-dir>
//
// It starts its own ghost-mcp, launches ghost-testbed (which Ghost places on a
// hidden desktop under the default background policy), types and clicks through
// the same verbs an agent uses, and saves each screenshot plus the call and its
// response to <out-dir>/steps.json. Nothing touches the user's windows, the
// clipboard or the emergency stop. scripts/demo-gif.py turns the output into
// assets/demo.gif.
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const [exe, testbed, out] = process.argv.slice(2);
if (!exe || !testbed || !out) {
  console.error('usage: node scripts/demo-capture.mjs <ghost-mcp.exe> <ghost-testbed.exe> <out-dir>');
  process.exit(2);
}
mkdirSync(out, { recursive: true });

// Pin the default background policy: an inherited GHOST_FOCUS_POLICY would
// start the testbed on the user's desktop and aim every later call at the
// user's foreground window.
const env = { ...process.env, GHOST_FOCUS_POLICY: 'background' };
delete env.GHOST_FOCUS_LOCK;
const server = spawn(exe, [], { stdio: ['pipe', 'pipe', 'pipe'], env });
server.stdin.on('error', () => {});
const pending = new Map();
let buf = '';
server.stdout.on('data', (chunk) => {
  buf += chunk;
  let nl;
  while ((nl = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, nl).trim();
    buf = buf.slice(nl + 1);
    if (!line) continue;
    let m;
    try { m = JSON.parse(line); } catch { console.error(`not JSON-RPC: ${line.slice(0, 200)}`); continue; }
    if (m.id != null && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  }
});
server.stderr.on('data', () => {});
server.on('exit', (code) => {
  for (const settle of pending.values()) settle({ error: { message: `ghost-mcp exited (${code})` } });
  pending.clear();
});

let nextId = 1;
function rpc(method, params, timeoutMs = 30000) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, timeoutMs);
    pending.set(id, (m) => { clearTimeout(timer); resolve(m); });
    server.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
}

async function call(name, args) {
  const m = await rpc('tools/call', { name, arguments: args });
  if (m.error) throw new Error(`${name}: ${JSON.stringify(m.error)}`);
  const content = m.result?.content ?? [];
  const text = content.find((c) => c.type === 'text')?.text;
  const image = content.find((c) => c.type === 'image');
  let body = null;
  try { body = text ? JSON.parse(text) : null; } catch { body = { raw: text }; }
  body = body?.data ?? body;
  if (m.result?.isError) throw new Error(`${name}: ${text}`);
  return { body, image };
}

const steps = [];
const TITLE = 'Invoice Editor';
let launched = false;
async function step(label, name, args) {
  const { body } = await call(name, args);
  if (name === 'ghost_window') {
    if (body?.surface !== 'hidden') throw new Error(`testbed did not start on a hidden desktop: ${JSON.stringify(body)}`);
    launched = true;
    await call('ghost_wait', { for: 'element', name: 'Field', role: 'edit', timeout_ms: 10000 });
  }
  const shot = await call('ghost_screenshot', { max_dim: 0, jpeg_quality: 95 });
  const encoded = shot.image?.data ?? shot.body?.png_base64 ?? shot.body?.jpeg_base64;
  if (!encoded) throw new Error(`ghost_screenshot returned no image: ${JSON.stringify(shot.body).slice(0, 400)}`);
  const png = `step-${steps.length}.${shot.body?.jpeg_base64 && !shot.image ? 'jpg' : 'png'}`;
  writeFileSync(join(out, png), Buffer.from(encoded, 'base64'));
  steps.push({ label, call: { name, args }, response: body, png });
  console.log(label, JSON.stringify(body).slice(0, 200));
}

try {
  await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'demo-capture', version: '1' } });
  server.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');

  await step('Launch an app. Ghost starts it on a hidden desktop.', 'ghost_window',
    { op: 'launch', exe: `"${testbed}" --title "${TITLE}" --label "Draft"` });
  await step('Type into a field, in the background.', 'ghost_act',
    { name: 'Field', role: 'edit', action: 'type', text_input: 'INV-2041 approved' });
  await step('Click a button. Ghost checks it took effect.', 'ghost_act',
    { name: 'Note', action: 'click' });
  await step('Click again. The window title proves it.', 'ghost_act',
    { name: 'Increment', action: 'click' });

  const stats = await call('ghost_stats', {});
  writeFileSync(join(out, 'steps.json'), JSON.stringify({ steps, stats: stats.body }, null, 2));
} finally {
  // Close through the anchor (the testbed Ghost launched), never by title: a
  // title search could match one of the user's windows. The testbed's Quit
  // button ends its process. Skipped unless the testbed launched hidden and
  // anchored.
  if (launched) {
    await call('ghost_act', { name: 'Quit', action: 'click' }).catch((e) => console.error(`close failed: ${e.message}`));
  }
  server.stdin.end();
  setTimeout(() => server.kill(), 2000).unref();
}
