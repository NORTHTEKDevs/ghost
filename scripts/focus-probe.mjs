// Which background rung makes a Chromium window on the USER desktop activate?
//
// Launches Edge in guest mode OFF-SCREEN (never visible, not minimised, so the
// background verbs accept it), loads a local test page, warms the accessibility
// tree, then drives each rung ONE AT A TIME and records the user's foreground
// window before and after each call. Pair with focus-watch.ps1 for the WinEvent
// ground truth (FOREGROUND / FOCUS / HIDE with timestamps).
//
//   node focus-probe.mjs <ghost-mcp.exe> <focus-testbed.html> [edge.exe]
import { spawn } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const [exe, page] = process.argv.slice(2);
const edge = process.argv[4] || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
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
const sleep = ms => new Promise(r => setTimeout(r, ms));
const fgNow = async () => { const s = (await call('ghost_session_state', {})).data; return { hwnd: s.foreground_hwnd, title: s.foreground_window }; };
const stamp = () => new Date().toISOString().slice(11, 23);

await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'focus-probe', version: '0' } });
server.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');

const fg0 = await fgNow();
console.log(`${stamp()} user foreground before launch: ${fg0.hwnd} "${fg0.title}"`);
const url = pathToFileURL(page).href;
// A fresh profile directory per run: no restore prompt, no hand-off to a running Edge.
const profile = `${process.env.TEMP}\\ghost-focus-probe-${Date.now()}`;
const child = spawn(edge, [`--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--disable-sync', '--force-renderer-accessibility', '--disable-features=msEdgeSidebarV2,msHubApps,msImplicitSignin', '--window-position=1904,1064', '--window-size=900,700', '--new-window', url], { detached: true, stdio: 'ignore' });
// Almost entirely off the bottom-right edge (a 16 px corner stays on-screen):
// Chromium treats a fully off-screen window as occluded and stops serving the
// page's accessibility tree, which would make every web lookup miss.
child.unref();

let win = null;
for (let i = 0; i < 80 && !win; i++) {
  await sleep(250);
  const list = (await call('ghost_window', { op: 'list' })).data.windows ?? [];
  win = list.find(w => /Ghost Focus Testbed/i.test(w.name) && w.surface === 'user');
}
if (!win) { console.log('test window never appeared'); process.exit(1); }
const fg1 = await fgNow();
console.log(`${stamp()} test window ${win.hwnd} "${win.name}"; foreground after launch: ${fg1.hwnd} ${fg1.hwnd === win.hwnd ? '<-- LAUNCH TOOK THE FOREGROUND' : '(unchanged)'}`);
const W = win.name;
await sleep(2500);
// A new process's first window takes the foreground. Push it back out without
// touching anything else: minimise (the previous window regains the foreground),
// then restore it with SW_SHOWNOACTIVATE so it is visible, off-screen, inactive.
if ((await fgNow()).hwnd === win.hwnd) {
  const { execFileSync } = await import('node:child_process');
  const ps = `Add-Type -Namespace P -Name W -MemberDefinition '[DllImport(\"user32.dll\")] public static extern bool ShowWindow(IntPtr h, int n);'; [P.W]::ShowWindow([IntPtr]${win.hwnd}, 6) | Out-Null; Start-Sleep -Milliseconds 400; [P.W]::ShowWindow([IntPtr]${win.hwnd}, 4) | Out-Null`;
  execFileSync('powershell.exe', ['-NoProfile', '-Command', ps], { windowsHide: true });
  await sleep(600);
  const fg2 = await fgNow();
  const list = (await call('ghost_window', { op: 'list' })).data.windows ?? [];
  const st = list.find(w => w.hwnd === win.hwnd)?.state;
  console.log(`${stamp()} pushed test window out of the foreground: fg=${fg2.hwnd} "${fg2.title}" window state=${st}`);
  if (fg2.hwnd === win.hwnd || st !== 'normal') { console.log('could not stage a non-foreground, non-minimised test window; aborting'); await call('ghost_window', { op: 'state', name: W, state: 'close' }); process.exit(2); }
}
// Warm the accessibility tree: Chromium switches it on for the first UIA client.
for (let i = 0; i < 6; i++) {
  const see = await call('ghost_see', { window: W, mode: 'text', limit: 400 });
  if (/First name/.test(see.data.text ?? '')) { console.log(`${stamp()} accessibility tree warm after ${i + 1} read(s)`); break; }
  await sleep(700);
  if (i === 5) console.log(`${stamp()} WARNING: page text never showed the form; last read: ${(see.data.text ?? '').slice(0, 200)}`);
}

// Push the test window out of the foreground again (see above) so every rung is
// judged from the same starting state: test window visible, inactive.
async function stage() {
  if ((await fgNow()).hwnd !== win.hwnd) return true;
  const { execFileSync } = await import('node:child_process');
  const ps = `Add-Type -Namespace P -Name W -MemberDefinition '[DllImport(\"user32.dll\")] public static extern bool ShowWindow(IntPtr h, int n);'; [P.W]::ShowWindow([IntPtr]${win.hwnd}, 6) | Out-Null; Start-Sleep -Milliseconds 400; [P.W]::ShowWindow([IntPtr]${win.hwnd}, 4) | Out-Null`;
  execFileSync('powershell.exe', ['-NoProfile', '-Command', ps], { windowsHide: true });
  await sleep(600);
  return (await fgNow()).hwnd !== win.hwnd;
}

const results = [];
async function op(label, name, args) {
  if (!(await stage())) { console.log(`${stamp()} ${label.padEnd(44)} SKIPPED: could not stage an inactive test window`); return; }
  const before = await fgNow();
  const t0 = Date.now();
  const r = await call(name, args);
  await sleep(400);
  const after = await fgNow();
  const stole = after.hwnd === win.hwnd && before.hwnd !== win.hwnd;
  const d = r.data;
  results.push({ label, stole, ok: r.body.ok });
  const g = d.focus_guard ?? d.results?.map(s => s.focus_guard).find(Boolean);
  const guard = g ? ` guard: taken, handed back=${g.restored} in ${g.ms}ms` : '';
  console.log(`${stamp()} ${label.padEnd(44)} ok=${r.body.ok} ${Date.now() - t0}ms via=${d.via ?? d.method ?? d.address_bar ?? '-'} verified=${d.verified ?? d.title_changed ?? '-'} focus_preserved=${d.focus_preserved ?? '-'}${guard} fg ${before.hwnd}->${after.hwnd}${after.hwnd !== before.hwnd ? ` "${(after.title ?? '').slice(0, 40)}"` : ''} ${stole ? '<-- STOLE FOREGROUND' : ''}${r.body.ok === false ? ' ERR ' + String(r.body.error).slice(0, 220) : ''}`);
  return r;
}

await op('A  web input: type (ValuePattern rung)', 'ghost_act', { window: W, name: 'First name', role: 'edit', action: 'type', text_input: 'Ada' });
await op('B  web button: click (Invoke rung)', 'ghost_act', { window: W, name: 'Save changes', role: 'button', action: 'click' });
await op('C  web textarea: type (ValuePattern rung)', 'ghost_act', { window: W, name: 'Notes', role: 'edit', action: 'type', text_input: 'a note' });
await op('D  contenteditable: type (posted keys)', 'ghost_act', { window: W, name: 'Rich editor', action: 'type', text_input: 'rich' });
await op('E  web link: click (Invoke rung)', 'ghost_act', { window: W, name: 'Docs link', action: 'click' });
await op('F  posted click at button centre', 'ghost_run', { steps: [{ op: 'ghost_find', window: W, name: 'Retitle', role: 'button' }, { op: 'ghost_click_at', window: W, x: '${steps.0.center.x}', y: '${steps.0.center.y}' }] });
await op('G  posted key: x', 'ghost_key', { window: W, keys: 'x' });
await op('G2 posted click on the omnibox', 'ghost_run', { steps: [{ op: 'ghost_find', window: W, name: 'Address and search bar', role: 'edit' }, { op: 'ghost_click_at', window: W, x: '${steps.0.center.x}', y: '${steps.0.center.y}' }] });
await op('H  omnibox: type URL (ValuePattern only)', 'ghost_act', { window: W, name: 'Address and search bar', role: 'edit', action: 'type', text_input: url + '?second' });
await sleep(800);
await op('I  omnibox: posted Enter', 'ghost_key', { window: W, keys: 'Enter' });
await sleep(1500);
await op('J  read text', 'ghost_see', { window: W, mode: 'text', limit: 200 });

const stolen = results.filter(r => r.stole).map(r => r.label.slice(0, 1));
console.log(`\nforeground stolen by ${stolen.length} of ${results.length} calls${stolen.length ? ': ' + stolen.join(', ') : ''}`);
await call('ghost_window', { op: 'state', name: W, state: 'close' });
// Edge keeps background processes alive after its last window closes; end the
// throwaway profile's processes so nothing lingers.
{
  const { execFileSync } = await import('node:child_process');
  const ps = `Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" | Where-Object { $_.CommandLine -match 'ghost-focus-probe' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`;
  try { execFileSync('powershell.exe', ['-NoProfile', '-Command', ps], { windowsHide: true }); } catch (e) { console.error(`cleanup failed: ${e.message}`); }
}
server.stdin.end();
setTimeout(() => process.exit(0), 500);
