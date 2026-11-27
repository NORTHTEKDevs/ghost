// MCP stdio driver for the 0.24.3 E2E run. node e2e.mjs <ghost-mcp.exe> <steps.json>
// steps.json: { "env": {...}, "steps": [ ... ] }. A step is one of
//   {"tool": name, "args": {...}, "expect": "ok"|"err", "contains": "text", "capture": {"name": "re"}}
//   {"rpc": method, "params": {...}, "contains": "text"}   (raw JSON-RPC, full reply printed)
//   {"exec": "<command>", "contains": "text"}               (a terminal command between calls)
//   {"sleep": ms}
// "{{name}}" in args / exec is replaced with an earlier capture. Each step prints PASS/FAIL
// against its expectation; the summary line counts them.
import { spawn, execSync } from 'node:child_process';
import fs from 'node:fs';
const [exe, stepsFile] = process.argv.slice(2);
const spec = JSON.parse(fs.readFileSync(stepsFile, 'utf8'));
const env = { ...process.env, GHOST_FOCUS_LOCK: 'on', GHOST_FOCUS_POLICY: 'background', ...(spec.env || {}) };
const p = spawn(exe, [], { env, stdio: ['pipe', 'pipe', 'pipe'] });
console.log(`# server pid ${p.pid}, env ${JSON.stringify(spec.env || {})}`);
let buf = ''; const waiters = new Map(); let id = 0; const vars = {};
p.stdout.on('data', d => { buf += d; let i; while ((i = buf.indexOf('\n')) >= 0) { const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1); if (!line) continue; try { const m = JSON.parse(line); if (m.id != null && waiters.has(m.id)) { waiters.get(m.id)(m); waiters.delete(m.id); } } catch { /* log noise on stdout is not a reply */ } } });
p.stderr.on('data', () => {});
const rpc = (method, params, ms = 90000) => new Promise(res => { const myId = ++id; waiters.set(myId, res); p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: myId, method, params }) + '\n'); setTimeout(() => { if (waiters.has(myId)) { waiters.delete(myId); res({ timeout: true }); } }, ms); });
const sub = (s) => s.replace(/\{\{(\w+)\}\}/g, (_, k) => vars[k] ?? `{{${k}}}`);
let pass = 0, fail = 0;
const judge = (s, text, isErr) => {
  let ok = true;
  if (s.expect === 'ok' && isErr) ok = false;
  if (s.expect === 'err' && !isErr) ok = false;
  if (s.contains && !text.includes(s.contains)) ok = false;
  if (s.absent && text.includes(s.absent)) ok = false;
  if (s.expect || s.contains || s.absent) { ok ? pass++ : fail++; return ok ? 'PASS' : 'FAIL'; }
  return 'INFO';
};
await rpc('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'e2e-0.24.3', version: '1' } });
p.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
for (const s of spec.steps) {
  if (s.note) { console.log(`\n## ${s.note}`); continue; }
  if (s.sleep) { await new Promise(r => setTimeout(r, s.sleep)); continue; }
  if (s.exec) {
    const cmd = sub(s.exec); let out, bad = false;
    try { out = execSync(cmd, { env: process.env, encoding: 'utf8', shell: 'cmd.exe' }); } catch (e) { bad = true; out = `exit ${e.status}: ${e.stdout}${e.stderr}`; }
    for (const [k, re] of Object.entries(s.capture || {})) { const m = out.match(new RegExp(re)); if (m) vars[k] = m[1]; }
    console.log(`$$ ${cmd}\n${out.trim().slice(0, 1200)}\n[${judge(s, out, bad)}]\n`); continue;
  }
  const t0 = Date.now();
  if (s.rpc) {
    const r = await rpc(s.rpc, JSON.parse(sub(JSON.stringify(s.params ?? {}))));
    const text = JSON.stringify(r);
    console.log(`>>> rpc ${s.rpc} ${JSON.stringify(s.params ?? {})}  (${Date.now() - t0}ms)\n<<< ${text.slice(0, s.max || 600)}\n[${judge(s, text, !!r.error)}]\n`); continue;
  }
  const args = JSON.parse(sub(JSON.stringify(s.args || {})));
  const r = await rpc('tools/call', { name: s.tool, arguments: args }, s.timeout || 90000);
  let text, isErr;
  if (r.error) { text = `RPC-ERR ${r.error.code}: ${r.error.message} ${JSON.stringify(r.error.data ?? '')}`; isErr = true; }
  else if (r.timeout) { text = 'TIMEOUT'; isErr = true; }
  else {
    const raw = r.result?.content?.map(c => c.text ?? `[${c.type}]`).join(' ') ?? JSON.stringify(r.result);
    isErr = !!r.result?.isError;
    try { const t = JSON.parse(raw); if (t.ok === false) isErr = true; text = t.ok === false ? `ERR ${t.error_code}: ${t.error} | hint: ${t.suggested_action}` : `ok ${JSON.stringify(t.data ?? t)}`; } catch { text = (isErr ? 'ERR ' : 'ok ') + raw; }
  }
  for (const [k, re] of Object.entries(s.capture || {})) { const m = text.match(new RegExp(re)); if (m) vars[k] = m[1]; }
  console.log(`>>> ${s.tool} ${JSON.stringify(args)}  (${Date.now() - t0}ms)\n<<< ${text.slice(0, s.max || 500)}\n[${judge(s, text, isErr)}]\n`);
}
console.log(`\n# summary: ${pass} pass, ${fail} fail`);
p.stdin.end(); setTimeout(() => { try { p.kill(); } catch { /* already gone */ } process.exit(0); }, 2500);
