// Does a ghost_shell child that is a console program pop a console window on the
// user's desktop (and take the foreground)? Runs two console children through a
// given ghost-mcp binary and reports the user's foreground before and after, plus
// any FOREGROUND events the focus watcher logged meanwhile.
//   node shell-console-probe.mjs <ghost-mcp.exe>
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';

const exe = process.argv[2];
// detached: the server gets NO console, which is how an MCP host starts it.
// Without this it would inherit this node process's console and the children
// under test would inherit that too, hiding the very effect being measured.
const server = spawn(exe, [], { stdio: ['pipe', 'pipe', 'pipe'], detached: true });
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
  const m = await rpc('tools/call', { name, arguments: args });
  const text = m.result?.content?.[0]?.text ?? JSON.stringify(m.error ?? m);
  let body; try { body = JSON.parse(text); } catch { body = { raw: text }; }
  return { body, data: body.data ?? body };
};
const fg = async () => { const s = (await call('ghost_session_state', {})).data; return `${s.foreground_hwnd} "${(s.foreground_window ?? '').slice(0, 40)}"`; };
const stamp = () => new Date().toTimeString().slice(0, 8);

await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'shell-console-probe', version: '0' } });
server.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
const t0 = stamp();
console.log(`${t0} foreground before: ${await fg()}`);
const windows = async () => ((await call('ghost_window', { op: 'list' })).data.windows ?? []).map(w => `${w.hwnd}:${w.name}`);
const consoleish = /node\.EXE|python|cmd\.exe|timeout|conhost|Windows Terminal|powershell/i;
for (const [label, args] of [
  ['node child (console program) for 1.5 s', { op: 'run', cmd: 'node -e "setTimeout(()=>{},1500)"', timeout_ms: 15000 }],
  ['cmd child (timeout /t 2)', { op: 'run', cmd: 'cmd /c timeout /t 2 /nobreak', timeout_ms: 15000 }],
  ['python child (time.sleep 1.5)', { op: 'run', cmd: 'python -c "import time; time.sleep(1.5)"', timeout_ms: 15000 }],
]) {
  const before = await fg();
  const winsBefore = new Set(await windows());
  const pendingRun = call('ghost_shell', args);
  await new Promise(r => setTimeout(r, 600));
  const during = (await windows()).filter(w => !winsBefore.has(w));
  const r = await pendingRun;
  const after = await fg();
  const popped = during.filter(w => consoleish.test(w));
  console.log(`${stamp()} ${label.padEnd(40)} ok=${r.body.ok} exit=${r.data.exit_code} new windows while running: ${during.length}${during.length ? ' [' + during.map(w => w.slice(0, 50)).join(' | ') + ']' : ''}${popped.length ? '  CONSOLE WINDOW POPPED' : ''} fg ${before} -> ${after}${before !== after ? '   <-- FOREGROUND CHANGED' : ''}`);
}
const log = readFileSync(process.env.TEMP + '\\ghost-focus-watch.log', 'utf8').split('\n').filter(l => l >= t0 && /FOREGROUND/.test(l));
console.log(`watcher FOREGROUND events since ${t0}: ${log.length}`);
for (const l of log.slice(0, 12)) console.log('  ' + l.slice(0, 150));
server.stdin.end();
setTimeout(() => process.exit(0), 300);
