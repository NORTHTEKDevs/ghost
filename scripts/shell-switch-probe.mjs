// Does the "Allow shell commands" setting actually stop the shell?
//
//   node scripts/shell-switch-probe.mjs <ghost-mcp.exe>
//
// The MCP bundle exposes GHOST_SHELL as a checkbox, and a checkbox writes
// "true"/"false", not "off". This runs a real server for each spelling and asks
// it to run a command, so the answer is the server's behaviour rather than the
// manifest's intent. Exits non-zero if any case is wrong.
import { spawn } from 'node:child_process';

const exe = process.argv[2];
if (!exe) { console.error('usage: node scripts/shell-switch-probe.mjs <ghost-mcp.exe>'); process.exit(2); }
let failures = 0;

function server(env) {
  const s = spawn(exe, [], { stdio: ['pipe', 'pipe', 'pipe'], env: { ...process.env, ...env } });
  let buf = '', err = '';
  const pending = new Map();
  s.stdout.on('data', d => {
    buf += d.toString();
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const l = buf.slice(0, i); buf = buf.slice(i + 1);
      if (!l.trim()) continue;
      try { const m = JSON.parse(l); if (m.id != null && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } } catch { process.stderr.write(`ghost-mcp stdout line is not JSON-RPC: ${l}\n`); }
    }
  });
  s.stderr.on('data', d => { err += d.toString(); });
  let id = 0;
  const rpc = (method, params) => new Promise((res, rej) => {
    const n = ++id;
    const t = setTimeout(() => rej(new Error('timeout ' + method)), 40000);
    pending.set(n, m => { clearTimeout(t); res(m); });
    s.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: n, method, params }) + '\n');
  });
  return { s, rpc, err: () => err };
}

async function check(label, env, expectShell) {
  const { s, rpc, err } = server(env);
  await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'shell-switch-probe', version: '0' } });
  s.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
  const m = await rpc('tools/call', { name: 'ghost_shell', arguments: { op: 'run', cmd: 'node -e "console.log(6*7)"' } });
  const text = m.result?.content?.[0]?.text ?? JSON.stringify(m.error ?? m);
  let b; try { b = JSON.parse(text); } catch { b = { raw: text }; }
  const ran = /42/.test(JSON.stringify(b.data ?? b));
  const refused = /disabled/i.test(text);
  const logged = err().replace(/\x1b\[[0-9;]*m/g, '').split('\n').find(l => /capabilities/.test(l)) ?? '';
  const shellField = (logged.match(/shell=(\w+)/) ?? [])[1];
  const ok = expectShell ? ran : refused;
  if (!ok) failures++;
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${label.padEnd(28)} ran=${ran} refused=${refused} startup-log shell=${shellField}`);
  s.stdin.end();
  s.kill();
}

await check('unset (default: shell on)', { GHOST_SHELL: undefined }, true);
await check('checkbox ticked ("true")', { GHOST_SHELL: 'true' }, true);
await check('checkbox cleared ("false")', { GHOST_SHELL: 'false' }, false);
await check('documented "off"', { GHOST_SHELL: 'off' }, false);
await check('"0"', { GHOST_SHELL: '0' }, false);

console.log(failures === 0 ? 'ALL PASS' : `${failures} FAILED`);
process.exit(failures === 0 ? 0 : 1);
