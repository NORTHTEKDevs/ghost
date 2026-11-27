// Live check of the focus-policy lock (0.22) against a real ghost-mcp binary
// over stdio. Touches nothing on the desktop: it only asks the server about its
// policy and tries to change it.
//
//   node scripts/focus-lock-probe.mjs <path-to-ghost-mcp.exe>
//
// Run 1 (default environment): the server must start locked to background,
// refuse prefer_background and foreground with an error that names
// GHOST_FOCUS_LOCK and the hidden-desktop route, and still accept background.
// Run 2 (GHOST_FOCUS_LOCK=off): the same calls must succeed.
// Exit code 0 only when every expectation holds.
import { spawn } from 'node:child_process';

const exe = process.argv[2];
if (!exe) { console.error('usage: node scripts/focus-lock-probe.mjs <ghost-mcp.exe>'); process.exit(2); }

let failures = 0;
const expect = (cond, label, detail = '') => {
  console.log(`${cond ? 'PASS' : 'FAIL'}  ${label}${detail ? '  ' + detail : ''}`);
  if (!cond) failures++;
};

async function withServer(env, body) {
  const server = spawn(exe, [], { stdio: ['pipe', 'pipe', 'pipe'], env: { ...process.env, ...env } });
  let buf = '';
  let stderr = '';
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
  server.stderr.on('data', d => { stderr += d.toString(); });
  let nextId = 1;
  const rpc = (method, params, timeoutMs = 20000) => new Promise((resolve, reject) => {
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
  const init = await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'focus-lock-probe', version: '0' } });
  server.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
  try {
    await body({ call, version: init.result?.serverInfo?.version, stderr: () => stderr });
  } finally {
    server.kill();
  }
}

console.log('--- run 1: default environment (expect locked)');
await withServer({ GHOST_FOCUS_LOCK: undefined, GHOST_FOCUS_POLICY: undefined }, async ({ call, version, stderr }) => {
  console.log(`server version ${version}`);
  const p = await call('ghost_focus_policy');
  expect(p.data.policy === 'background', 'starts in background', `policy=${p.data.policy}`);
  expect(p.data.locked === true, 'reports locked:true', `locked=${p.data.locked}`);
  for (const policy of ['prefer_background', 'foreground']) {
    const r = await call('ghost_set_focus_policy', { policy });
    expect(!r.ok, `refuses ${policy}`, r.ok ? JSON.stringify(r.data).slice(0, 120) : '');
    expect(/GHOST_FOCUS_LOCK/.test(r.error), `  refusal names GHOST_FOCUS_LOCK`);
    expect(/op=launch/.test(r.error), `  refusal names the hidden-desktop route`);
    const after = await call('ghost_focus_policy');
    expect(after.data.policy === 'background', `  policy unchanged after refused ${policy}`, `policy=${after.data.policy}`);
  }
  const bg = await call('ghost_set_focus_policy', { policy: 'background' });
  expect(bg.ok && bg.data.policy === 'background', 'background is still accepted');
  const st = await call('ghost_session_state');
  expect(st.data.focus_locked === true, 'ghost_session_state reports focus_locked:true', `focus_locked=${st.data.focus_locked}`);
  // tracing colours its output even into a pipe; strip the escapes before matching.
  const plain = stderr().replace(/\x1b\[[0-9;]*m/g, '');
  expect(/capabilities .*locked=true/.test(plain), 'startup log line says locked=true');
  expect(/capabilities .*shell=(true|false)/.test(plain), 'startup log line states whether the shell is enabled');
});

console.log('--- run 2: GHOST_FOCUS_LOCK=off (operator unlocked)');
await withServer({ GHOST_FOCUS_LOCK: 'off' }, async ({ call }) => {
  const p = await call('ghost_focus_policy');
  expect(p.data.locked === false, 'reports locked:false', `locked=${p.data.locked}`);
  const fg = await call('ghost_set_focus_policy', { policy: 'foreground' });
  expect(fg.ok && fg.data.policy === 'foreground', 'foreground accepted when unlocked', fg.error);
  const back = await call('ghost_set_focus_policy', { policy: 'background' });
  expect(back.ok && back.data.policy === 'background', 'back to background');
});

console.log('--- run 3: GHOST_FOCUS_POLICY=foreground with the lock on (operator pre-selects)');
await withServer({ GHOST_FOCUS_POLICY: 'foreground' }, async ({ call }) => {
  const p = await call('ghost_focus_policy');
  expect(p.data.policy === 'foreground' && p.data.locked === true, 'operator env honoured, still locked', `policy=${p.data.policy} locked=${p.data.locked}`);
  const bg = await call('ghost_set_focus_policy', { policy: 'background' });
  expect(bg.ok, 'agent may lower to background');
  const up = await call('ghost_set_focus_policy', { policy: 'foreground' });
  expect(!up.ok, 'but cannot raise it back');
});

console.log(failures === 0 ? 'ALL PASS' : `${failures} FAILED`);
process.exit(failures === 0 ? 0 : 1);
