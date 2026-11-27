// MCP stdio driver (as ~/ghost-repro/mcp.mjs) plus {"exec": "<command>"} steps
// that run a terminal command between calls. node mcp2.mjs <exe> <steps.json>
import { spawn, execSync } from 'node:child_process';
import fs from 'node:fs';
const [exe, stepsFile] = process.argv.slice(2);
const steps = JSON.parse(fs.readFileSync(stepsFile, 'utf8'));
const env = { ...process.env, GHOST_FOCUS_LOCK: 'on', GHOST_FOCUS_POLICY: 'background', GHOST_SHELL: 'off' };
const p = spawn(exe, [], { env, stdio: ['pipe', 'pipe', 'pipe'] });
let buf = ''; const waiters = new Map(); let id = 0; let lastTab = null;
p.stdout.on('data', d => { buf += d; let i; while ((i = buf.indexOf('\n')) >= 0) { const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1); if (!line) continue; try { const m = JSON.parse(line); if (m.id != null && waiters.has(m.id)) { waiters.get(m.id)(m); waiters.delete(m.id); } } catch { /* a non-JSON line (server log noise): not a response, skip it */ } } });
p.stderr.on('data', () => {});
const rpc = (method, params) => new Promise(res => { const myId = ++id; waiters.set(myId, res); p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: myId, method, params }) + '\n'); setTimeout(() => { if (waiters.has(myId)) { waiters.delete(myId); res({ timeout: true }); } }, 60000); });
const brief = (r) => { try { const t = JSON.parse(r.result.content[0].text); return t.ok === false ? `ERR ${t.error_code}: ${t.error}\n    hint: ${t.suggested_action}` : `ok ${JSON.stringify(t.data).slice(0, 160)}`; } catch { return JSON.stringify(r).slice(0, 300); } };
await rpc('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'repro', version: '1' } });
p.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
for (const s of steps) {
  if (s.sleep) { await new Promise(r => setTimeout(r, s.sleep)); continue; }
  if (s.exec) { let out; try { out = execSync(s.exec, { env: process.env, encoding: 'utf8' }); } catch (e) { out = `exit ${e.status}: ${e.stdout}${e.stderr}`; } console.log(`$$ ${s.exec}\n${out.trim()}\n`); continue; }
  const t0 = Date.now();
  // "$tab" in an argument is the tab id an earlier call returned (data.tab).
  const args = JSON.parse(JSON.stringify(s.args || {}).replaceAll('"$tab"', JSON.stringify(lastTab)));
  const r = await rpc('tools/call', { name: s.tool, arguments: args });
  try { const t = JSON.parse(r.result.content[0].text); if (t.data && t.data.tab) lastTab = t.data.tab; } catch { /* not a JSON tool result: nothing to remember */ }
  console.log(`>>> ${s.tool} ${JSON.stringify(args)}  (${Date.now() - t0}ms)\n<<< ${brief(r)}\n`);
}
p.stdin.end(); setTimeout(() => process.exit(0), 1500);
