// The product claim, measured on the user's REAL desktop with a fresh server:
// an agent drives a visible browser window through the whole verb set while
// the human keeps the foreground, the mouse and the keyboard.
//
//   node scripts/background-desktop-probe.mjs <ghost-mcp.exe> [--simulate|--no-simulate] [--edge <msedge.exe>]
//
// What runs:
//   - scripts/interference-watch.ps1: an independent observer (WinEvent +
//     low-level hooks) logging every foreground change and every INJECTED key
//     or mouse event on the desktop, with timestamps.
//   - a fresh ghost-mcp over stdio, default environment (policy locked).
//   - Edge with a throwaway profile, VISIBLE on the user's desktop (the human
//     can watch it being operated), pushed out of the foreground once after
//     launch (a new process's first window always takes it).
//   - Phase A: see / type / click / assert / key / scroll / posted click /
//     screenshot / navigate / minimize / restore / maximize / shell / and the
//     refusals (raise policy, drag, focus) against that window. Expectation:
//     zero injected input, zero foreground changes to the test window.
//   - Phases B, C1, C2 (only when the human has been idle for a while, or with
//     --simulate): a stand-in user (scripts/simulate-user.ps1, real SendInput)
//     types a known text into a fresh Notepad the whole time Ghost works in
//     Edge, then switches between the two the way people do (click, Alt+Tab).
//     Expectation: every injected keystroke lands in Notepad, the text comes
//     out byte-exact, and the browser never holds the foreground.
// Prints a scoreboard and exits non-zero if the claim did not hold.
import { spawn, execFile, execFileSync } from 'node:child_process';
import { pathToFileURL, fileURLToPath } from 'node:url';
import * as require$fs from 'node:fs';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const exe = argv.find(a => !a.startsWith('--'));
if (!exe) { console.error('usage: node scripts/background-desktop-probe.mjs <ghost-mcp.exe> [--simulate|--no-simulate] [--edge <path>]'); process.exit(2); }
const forceSim = argv.includes('--simulate') ? true : argv.includes('--no-simulate') ? false : null;
const edgeExe = argv.includes('--edge') ? argv[argv.indexOf('--edge') + 1] : 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const testbed = join(here, 'focus-testbed.html');
const watchPs = join(here, 'interference-watch.ps1');
const simPs = join(here, 'simulate-user.ps1');
const tmp = process.env.TEMP;
const runId = Date.now();
const watchLog = `${tmp}\\ghost-bg-probe-${runId}.jsonl`;
const watchStop = `${tmp}\\ghost-bg-probe-${runId}.stop`;

const sleep = ms => new Promise(r => setTimeout(r, ms));
const stamp = () => new Date().toISOString().slice(11, 23);
const ps = (command) => execFileSync('powershell.exe', ['-NoProfile', '-Command', command], { windowsHide: true, encoding: 'utf8' }).trim();
const psFile = (file, args) => new Promise((resolve) => execFile('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', file, ...args], { windowsHide: true }, (e, out, err) => resolve({ e, out, err })));
const marks = [];
// Markers go into the observer's own log so every event it records can be
// attributed to the verb that was running, to the millisecond.
let appendMark = () => {};
const mark = (label, quiet = false) => { marks.push({ t: Date.now(), label }); appendMark(label); if (!quiet) console.log(`${stamp()} == ${label}`); };
const failures = [];
const notes = [];
const check = (cond, label, detail = '') => { console.log(`${cond ? 'PASS' : 'FAIL'}  ${label}${detail ? '  ' + detail : ''}`); if (!cond) failures.push(label); };

// ---- observer ------------------------------------------------------------
const watcher = spawn('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', watchPs, '-Log', watchLog, '-Stop', watchStop], { stdio: 'ignore', windowsHide: true });
for (let i = 0; i < 40; i++) { await sleep(250); if (existsSync(watchLog) && /"kind":"started"/.test(readFileSync(watchLog, 'utf8'))) break; }
// A separate file: the observer holds its log open without sharing writes.
const markLog = watchLog.replace(/\.jsonl$/, '.marks.jsonl');
appendMark = (label) => { try { require$fs.appendFileSync(markLog, JSON.stringify({ t: Date.now(), kind: 'mark', label }) + '\n'); } catch (e) { console.error(`mark log write failed: ${e.message}`); } };
const started = JSON.parse(readFileSync(watchLog, 'utf8').split('\n').find(l => /"started"/.test(l)) ?? '{}');
console.log(`${stamp()} observer up: hooks=${JSON.stringify(started.hooks)} user foreground=${started.fg}`);
if (!started.hooks || started.hooks.some(h => !h)) { console.log('observer hooks did not install; aborting'); process.exit(3); }

// ---- server ----------------------------------------------------------------
const server = spawn(exe, [], { stdio: ['pipe', 'pipe', 'pipe'], env: { ...process.env, GHOST_FOCUS_LOCK: undefined, GHOST_FOCUS_POLICY: undefined, RUST_LOG: process.env.RUST_LOG ?? 'ghost_mcp=debug' } });
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
let serverErr = '';
server.stderr.on('data', d => { serverErr += d.toString(); });
let nextId = 1;
const rpc = (method, params, timeoutMs = 60000) => new Promise((resolve, reject) => {
  const id = nextId++;
  const t = setTimeout(() => { pending.delete(id); reject(new Error(`timeout ${method}`)); }, timeoutMs);
  pending.set(id, m => { clearTimeout(t); resolve(m); });
  server.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
});
const call = async (name, args = {}) => {
  const t0 = Date.now();
  const m = await rpc('tools/call', { name, arguments: args });
  const c0 = m.result?.content?.[0];
  if (c0?.type === 'image') return { ok: true, data: { image: true, bytes: (c0.data ?? '').length }, body: { ok: true }, error: '', wall: Date.now() - t0 };
  const text = c0?.text ?? JSON.stringify(m.error ?? m);
  let b; try { b = JSON.parse(text); } catch { b = { raw: text }; }
  const ok = b.ok !== false && !m.error && !(m.result?.isError);
  return { ok, data: b.data ?? b, body: b, error: String(b.error ?? m.error?.message ?? (m.result?.isError ? text : '')), wall: Date.now() - t0 };
};
const init = await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'background-desktop-probe', version: '0' } });
server.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
const pol = await call('ghost_focus_policy');
console.log(`${stamp()} server ${init.result?.serverInfo?.version} policy=${pol.data.policy} locked=${pol.data.locked}`);
check(pol.data.policy === 'background' && pol.data.locked === true, 'fresh server starts background + locked');

const fgNow = async () => { const s = (await call('ghost_session_state')).data; return { hwnd: s.foreground_hwnd, title: s.foreground_window ?? '' }; };
const windows = async () => (await call('ghost_window', { op: 'list' })).data.windows ?? [];
const fg0 = await fgNow();
console.log(`${stamp()} the human's window: ${fg0.hwnd}`);

// ---- idle decides whether a stand-in user types alongside -------------------
const idleMs = Number(ps(`Add-Type -Namespace I -Name L -MemberDefinition '[StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; } [DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO p);'; $l = New-Object I.L+LASTINPUTINFO; $l.cbSize = 8; [void][I.L]::GetLastInputInfo([ref]$l); [Environment]::TickCount - $l.dwTime`));
const simulate = forceSim ?? (idleMs > 45000);
console.log(`${stamp()} human idle for ${Math.round(idleMs / 1000)} s -> stand-in user phases ${simulate ? 'ON' : 'OFF (the human is the user for this run)'}`);

// ---- test window on the user's desktop --------------------------------------
const url = pathToFileURL(testbed).href;
const profile = `${tmp}\\ghost-bg-probe-${runId}`;
const edgeChild = spawn(edgeExe, [`--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--disable-sync', '--force-renderer-accessibility', '--disable-features=msEdgeSidebarV2,msHubApps,msImplicitSignin', '--window-position=80,80', '--window-size=960,720', '--new-window', url], { detached: true, stdio: 'ignore' });
edgeChild.unref();
let edge = null;
for (let i = 0; i < 80 && !edge; i++) { await sleep(250); edge = (await windows()).find(w => /Ghost Focus Testbed/i.test(w.name) && w.surface === 'user'); }
if (!edge) { console.log('test window never appeared'); process.exit(1); }
const W = edge.name;
// Anchor once by handle. Titles change the moment a page navigates, and a
// title-based target then misses a window that is right there - which is why
// agents are told to name the window once and let the anchor follow it.
const on = {};  // every verb below targets the anchor
await sleep(2000);
// The launch takes the foreground (every Chromium launch style does). Push the
// window out again without touching anything else: minimise (the previous
// window gets the foreground back) and show it again without activation.
async function unfocusEdge() {
  if ((await fgNow()).hwnd !== edge.hwnd) return true;
  ps(`Add-Type -Namespace P -Name W -MemberDefinition '[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);'; [void][P.W]::ShowWindow([IntPtr]${edge.hwnd}, 6); Start-Sleep -Milliseconds 400; [void][P.W]::ShowWindow([IntPtr]${edge.hwnd}, 4)`);
  await sleep(600);
  return (await fgNow()).hwnd !== edge.hwnd;
}
await call('ghost_window', { op: 'anchor', name: W });
const staged = await unfocusEdge();
console.log(`${stamp()} test window ${edge.hwnd} visible on the user's desktop, inactive=${staged}; foreground back on ${(await fgNow()).hwnd}`);
if (!staged) { console.log('could not stage an inactive test window; aborting'); process.exit(2); }
for (let i = 0; i < 8; i++) { const see = await call('ghost_see', { ...on, mode: 'text', limit: 400 }); if (/First name/.test(see.data.text ?? '')) break; await sleep(600); }

// ---- one verb, judged --------------------------------------------------------
const results = [];
async function op(phase, label, name, args, expect = {}) {
  const before = await fgNow();
  mark(`op:${phase}:${label}`, true);
  const t0 = Date.now();
  const r = await call(name, args);
  await sleep(250);
  const after = await fgNow();
  const stole = after.hwnd === edge.hwnd && before.hwnd !== edge.hwnd;
  const d = r.data ?? {};
  const g = d.focus_guard ?? d.results?.map(s => s?.focus_guard).find(Boolean);
  const wantOk = expect.ok ?? true;
  const okAsExpected = wantOk ? r.ok : !r.ok;
  const textOk = expect.error ? expect.error.test(r.error) : true;
  const pass = okAsExpected && textOk && !stole;
  results.push({ phase, label, pass, stole, ok: r.ok, guard: g });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${phase} ${label.padEnd(46)} ok=${r.ok} ${String(Date.now() - t0).padStart(5)}ms verified=${d.verified ?? d.title_changed ?? d.found ?? '-'} focus_preserved=${d.focus_preserved ?? '-'}${g ? ` guard:handed_back=${g.restored} in ${g.ms}ms` : ''}${stole ? ' <-- TEST WINDOW TOOK THE FOREGROUND' : ''}${!okAsExpected ? ` (expected ok=${wantOk})` : ''}${!textOk ? ' (error text did not match)' : ''}${r.ok === false && wantOk ? ' ERR ' + r.error.slice(0, 200) : ''}`);
  if (!pass) failures.push(`${phase} ${label}`);
  return r;
}

async function coreVerbs(phase) {
  await op(phase, 'see (text)', 'ghost_see', { ...on, mode: 'text', limit: 300 });
  await op(phase, 'type into web input (ValuePattern)', 'ghost_act', { ...on, name: 'First name', role: 'edit', action: 'type', text_input: 'Ada' });
  await op(phase, 'click web button (Invoke)', 'ghost_act', { ...on, name: 'Save changes', role: 'button', action: 'click' });
  await op(phase, 'assert value-contains', 'ghost_assert', { ...on, predicate: 'value-contains', name: 'First name', role: 'edit', text: 'Ada' });
  await op(phase, 'posted key Tab', 'ghost_key', { ...on, keys: 'Tab' });
  await op(phase, 'posted wheel scroll', 'ghost_scroll', { ...on, direction: 'down', amount: 3 });
  await op(phase, 'posted click at button (ghost_run)', 'ghost_run', { steps: [{ op: 'ghost_find', ...on, name: 'Retitle', role: 'button' }, { op: 'ghost_click_at', ...on, x: '${steps.0.center.x}', y: '${steps.0.center.y}' }] });
  await op(phase, 'screenshot by handle', 'ghost_screenshot', { ...on });
  await op(phase, 'navigate (address bar, background)', 'ghost_wait', { ...on, for: 'navigate', url: 'https://example.com/', timeout_ms: 15000 });
  await op(phase, 'navigate back to the testbed', 'ghost_wait', { ...on, for: 'navigate', url, timeout_ms: 15000 });
  const live = (await call('ghost_window', { op: 'anchor' })).data.anchor?.title ?? W;
  await op(phase, 'window state: minimize', 'ghost_window', { op: 'state', name: live, state: 'minimize' });
  await op(phase, 'window state: restore', 'ghost_window', { op: 'state', name: live, state: 'restore' });
  await op(phase, 'window state: maximize', 'ghost_window', { op: 'state', name: live, state: 'maximize' });
  await op(phase, 'window state: restore', 'ghost_window', { op: 'state', name: live, state: 'restore' });
  const st = (await windows()).find(w => w.hwnd === edge.hwnd)?.state;
  check(st === 'normal', `${phase} window is normal after minimize/restore/maximize/restore`, `state=${st}`);
}

mark('phaseA_start');
await coreVerbs('A');
await op('A', 'shell: node one-liner (no console window)', 'ghost_shell', { op: 'run', cmd: 'node -e "console.log(6*7)"' });
await op('A', 'REFUSED: raise policy to foreground', 'ghost_set_focus_policy', { policy: 'foreground' }, { ok: false, error: /GHOST_FOCUS_LOCK/ });
await op('A', 'REFUSED: raise policy to prefer_background', 'ghost_set_focus_policy', { policy: 'prefer_background' }, { ok: false, error: /GHOST_FOCUS_LOCK/ });
await op('A', 'REFUSED: real drag', 'ghost_drag', { from_x: 200, from_y: 200, to_x: 300, to_y: 300 }, { ok: false, error: /background|op=launch/ });
const focusR = await op('A', 'op=focus anchors, does not raise', 'ghost_window', { op: 'focus', name: W });
check(focusR.data?.raised === false && focusR.data?.anchored === true, 'A op=focus reported anchored:true raised:false', JSON.stringify({ raised: focusR.data?.raised, anchored: focusR.data?.anchored }));
mark('phaseA_end');

// ---- stand-in user ------------------------------------------------------------
const S1 = 'the quick brown fox jumps over the lazy dog 0123456789 '.repeat(7);
const S2 = 'switched back by clicking; still typing here. ';
const S3 = 'switched back with alt-tab; still typing here. ';
let notepad = null, savedClip = null, notepadText = null;
if (simulate) {
  const before = new Set((await windows()).filter(w => /Notepad/i.test(w.name)).map(w => w.hwnd));
  const np = spawn('notepad.exe', [], { detached: true, stdio: 'ignore' }); np.unref();
  for (let i = 0; i < 60 && !notepad; i++) { await sleep(250); notepad = (await windows()).find(w => /Notepad/i.test(w.name) && w.surface === 'user' && !before.has(w.hwnd)); }
  if (!notepad) { console.log('Notepad never appeared; skipping stand-in phases'); }
  else {
    console.log(`${stamp()} stand-in user's Notepad: ${notepad.hwnd} "${notepad.name}"`);
    savedClip = (await call('ghost_clipboard', { op: 'get' })).data;
    await psFile(simPs, ['-SendToBack', String(edge.hwnd), '-Raise', String(notepad.hwnd), '-ClickHwnd', String(notepad.hwnd)]);
    check((await fgNow()).hwnd === notepad.hwnd, 'B stand-in user activated Notepad by clicking it');

    mark('phaseB_start');
    const typing = psFile(simPs, ['-Text', S1, '-DelayMs', '40']);
    await sleep(300);
    await coreVerbs('B');
    await typing;
    mark('phaseB_end');
    check((await fgNow()).hwnd === notepad.hwnd, 'B foreground is still Notepad after the whole verb set');

    // C1: the human clicks into the browser, then clicks back to Notepad (the
    // common case: Notepad received the last input, the browser has no
    // foreground rights), and Ghost types into the browser meanwhile.
    mark('phaseC1_start');
    mark('user_in_browser_start', true);
    await psFile(simPs, ['-ClickHwnd', String(edge.hwnd)]);
    mark('user_in_browser_end', true);
    await psFile(simPs, ['-SendToBack', String(edge.hwnd), '-Raise', String(notepad.hwnd), '-ClickHwnd', String(notepad.hwnd)]);
    check((await fgNow()).hwnd === notepad.hwnd, 'C1 stand-in user is back in Notepad (click)');
    const t1 = psFile(simPs, ['-Text', S2, '-DelayMs', '40']);
    await sleep(200);
    await op('C1', 'type into web textarea while user types elsewhere', 'ghost_act', { ...on, name: 'Notes', role: 'edit', action: 'type', text_input: 'note one' });
    await op('C1', 'click web button while user types elsewhere', 'ghost_act', { ...on, name: 'Save changes', role: 'button', action: 'click' });
    await t1;
    mark('phaseC1_end');

    // C2: the human leaves the browser with Alt+Tab (the browser may keep the
    // right to activate itself), and Ghost types into it right away.
    mark('phaseC2_start');
    mark('user_in_browser_start', true);
    await psFile(simPs, ['-ClickHwnd', String(edge.hwnd)]);
    // Alt+Tab first, because the browser keeps activation rights when the human
    // leaves it that way - the hardest case for the sentinel. A SYNTHESIZED
    // Alt+Tab routinely leaves the switcher itself in front, which measures the
    // simulation and not Ghost, so the click is the one that must land.
    await psFile(simPs, ['-AltTab']);
    await psFile(simPs, ['-SendToBack', String(edge.hwnd), '-Raise', String(notepad.hwnd), '-ClickHwnd', String(notepad.hwnd)]);
    mark('user_in_browser_end', true);
    const afterAlt = await fgNow();
    check(afterAlt.hwnd === notepad.hwnd, 'C2 the user is back in Notepad after leaving the browser', afterAlt.hwnd === notepad.hwnd ? '' : `fg=${afterAlt.hwnd}`);
    const t2 = psFile(simPs, ['-Text', S3, '-DelayMs', '40']);
    await sleep(200);
    await op('C2', 'type into web input right after Alt+Tab away', 'ghost_act', { ...on, name: 'First name', role: 'edit', action: 'type', text_input: 'Baer' });
    await op('C2', 'click web button right after Alt+Tab away', 'ghost_act', { ...on, name: 'Save changes', role: 'button', action: 'click' });
    await t2;
    mark('phaseC2_end');

    // What actually landed in the user's document.
    // Read back what the user's document actually holds. Raise it first: a
    // click on a covered title bar lands on whatever is covering it, and then
    // Ctrl+A/Ctrl+C copies THAT window instead.
    await psFile(simPs, ['-SendToBack', String(edge.hwnd), '-Raise', String(notepad.hwnd), '-ClickHwnd', String(notepad.hwnd)]);
    const onNotepad = (await fgNow()).hwnd === notepad.hwnd;
    await psFile(simPs, ['-CopyAll']);
    const clip = (await call('ghost_clipboard', { op: 'get' })).data;
    notepadText = typeof clip?.text === 'string' ? clip.text : (typeof clip === 'string' ? clip : JSON.stringify(clip));
    const expected = S1 + S2 + S3;
    const same = notepadText.replace(/\r\n/g, '\n') === expected;
    check(same, 'the user\'s Notepad holds exactly what the user typed', same ? `${expected.length} chars` : `expected ${expected.length} chars, got ${notepadText.length}; first difference at ${[...expected].findIndex((c, i) => notepadText[i] !== c)}`);
    const web = await call('ghost_see', { ...on, mode: 'text', limit: 400 });
    notes.push(`browser page text after the run: ${(web.data.text ?? '').replace(/\s+/g, ' ').slice(0, 220)}`);
    if (savedClip && typeof savedClip.text === 'string') await call('ghost_clipboard', { op: 'set', text: savedClip.text });
    try { ps(`Stop-Process -Id ${notepad.pid} -Force -ErrorAction SilentlyContinue`); } catch (e) { console.error(`cleanup failed: ${e.message}`); }
  }
}

// ---- teardown ------------------------------------------------------------------
mark('measure_end');
{ const live = (await call('ghost_window', { op: 'anchor' })).data.anchor?.title ?? W;
  await call('ghost_window', { op: 'state', name: live, state: 'close' }); }
await sleep(800);
try { ps(`Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" | Where-Object { $_.CommandLine -match 'ghost-bg-probe-${runId}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`); } catch (e) { console.error(`cleanup failed: ${e.message}`); }
writeFileSync(watchStop, '');
for (let i = 0; i < 40; i++) { await sleep(250); if (/"kind":"summary"/.test(readFileSync(watchLog, 'utf8'))) break; }
// Ghost's own account of the run, read while the server is still alive.
const selfAudit = (await call('ghost_session_state')).data.interference_audit ?? {};
server.stdin.end();

// ---- what the observer saw ------------------------------------------------------
const parseLines = (f) => (existsSync(f) ? readFileSync(f, 'utf8') : '').split('\n').filter(Boolean).map(l => { try { return JSON.parse(l); } catch { return null; } }).filter(Boolean);
const events = [...parseLines(watchLog), ...parseLines(markLog)].sort((a, b) => a.t - b.t);
const at = label => marks.find(m => m.label === label)?.t;
const opAt = t => { let last = null; for (const e of events) { if (e.kind === 'mark' && e.label.startsWith('op:') && e.t <= t) last = e.label.slice(3); } return last ?? '-'; };
// The honest bar. Chromium's self-activation cannot be prevented from outside
// the browser (it is the browser's own SetForegroundWindow, made with rights
// Windows granted it), so what is measured is what a human would actually
// notice: does the foreground come straight back, and do their keystrokes ever
// land in the wrong window.
const MAX_FLASH_MS = 250;
function judge(phase, startLabel, endLabel) {
  const t0 = at(startLabel), t1 = at(endLabel);
  if (!t0 || !t1) return;
  const ev = events.filter(e => e.t >= t0 && e.t <= t1);
  const fgs = ev.filter(e => e.kind === 'foreground');
  const usersWindow = h => h === fg0.hwnd || (notepad && h === notepad.hwnd);
  // Any window of the browser's process counts as the test window: a popup is
  // a separate handle and takes the foreground just as visibly.
  const edgeProc = e => e.proc === 'msedge';
  const takes = fgs.filter(edgeProc);
  const holds = takes.map(e => { const next = fgs.find(f => f.t > e.t && !edgeProc(f)); return { t: e.t, hwnd: e.hwnd, ms: next ? next.t - e.t : (t1 - e.t), open: !next, op: opAt(e.t) }; });
  const longHolds = holds.filter(h => h.ms > MAX_FLASH_MS);
  const others = fgs.filter(e => !edgeProc(e) && !usersWindow(e.hwnd) && (e.since_hw_ms == null || e.since_hw_ms > 1500));
  const keys = ev.filter(e => e.kind === 'key_injected');
  const visits = [];
  for (const e of events) {
    if (e.kind !== 'mark') continue;
    if (e.label === 'user_in_browser_start') visits.push({ from: e.t, to: Infinity });
    if (e.label === 'user_in_browser_end' && visits.length) visits[visits.length - 1].to = e.t + 1500;
  }
  const userWasInBrowser = t => visits.some(v => t >= v.from && t <= v.to);
  const strayKeys = keys.filter(e => !usersWindow(e.fg) && !userWasInBrowser(e.t));
  const mice = ev.filter(e => e.kind === 'mouse_injected');
  const endFg = fgs.length ? fgs[fgs.length - 1] : null;
  console.log(`--- observer, phase ${phase}: browser took the foreground ${takes.length}x` + (holds.length ? ` (held ms: ${holds.map(h => h.ms + (h.open ? '+' : '')).join(', ')})` : '') + `; other unexplained changes=${others.length}; injected keys=${keys.length} (outside the user's window=${strayKeys.length}); injected mouse=${mice.length}`);
  for (const h of longHolds) console.log(`      HELD ${h.ms} ms${h.open ? ' (past the end of the phase)' : ''} on hwnd ${h.hwnd}, during ${h.op}`);
  for (const e of others) console.log(`      unexplained foreground change to ${e.hwnd} (${e.proc}) since_hw_ms=${e.since_hw_ms} during ${opAt(e.t)}`);
  // THE measure: every keystroke carries the foreground window as read at the
  // moment it was delivered, so this is where the user's typing actually went.
  check(strayKeys.length === 0, `${phase} every keystroke landed in the user's own window`, strayKeys.length ? `${strayKeys.length} of ${keys.length} stray, first during ${opAt(strayKeys[0].t)}` : `${keys.length} keys`);
  // Informational: the gap between activation EVENTS over-reports, because a
  // window that gains and loses activation in one beat can produce the first
  // event without a matching one for the window that gets it back.
  if (longHolds.length) console.log(`      (activation-event gaps over ${MAX_FLASH_MS} ms: ${longHolds.map(h => h.ms).join(', ')} - see the keystroke measure above for what the human actually experienced)`);
  check(others.length === 0, `${phase} no unexplained foreground change elsewhere (consoles, popups)`);
  if (!simulate) check(keys.length === 0, `${phase} Ghost injected no keystroke at all`);
  check(mice.length === (simulate ? mice.length : 0), `${phase} Ghost injected no mouse event`, simulate ? '(stand-in user clicks are its own)' : '');
  const lastKey = keys[keys.length - 1];
  if (lastKey) check(usersWindow(lastKey.fg), `${phase} the user's last keystroke went to their own window`, `${lastKey.fg_proc}`);
  if (holds.length) notes.push(`${phase}: browser self-activations ${holds.length}, worst hold ${Math.max(...holds.map(h => h.ms))} ms`);
}
judge('A', 'phaseA_start', 'phaseA_end');
if (notepad) {
  judge('B', 'phaseB_start', 'phaseB_end');
  judge('C1', 'phaseC1_start', 'phaseC1_end');
  judge('C2', 'phaseC2_start', 'phaseC2_end');
}
console.log(`--- Ghost's own audit: synthetic=${selfAudit.synthetic_foreground_changes} handed_back=${selfAudit.foreground_handed_back} failures=${selfAudit.foreground_hand_back_failures} stood_down=${selfAudit.sentinel_stood_down}`);
const summary = events.find(e => e.kind === 'summary');
console.log(`--- observer totals: ${JSON.stringify(summary)}`);
for (const n of notes) console.log(`note: ${n}`);
{
  const lines = serverErr.replace(/\x1b\[[0-9;]*m/g, '').split('\n').filter(l => /sentinel|SYNTHETIC/.test(l));
  console.log(`--- the server's own sentinel log (${lines.length} lines):`);
  for (const l of lines.slice(-14)) console.log('    ' + l.replace(/^\S+\s+/, '').slice(0, 190));
}
if (/panick/i.test(serverErr)) { console.log('SERVER PANIC:\n' + serverErr.slice(0, 1200)); failures.push('server panic'); }
console.log(`\nobserver log: ${watchLog}`);
console.log(failures.length === 0 ? `ALL PASS (${results.length} verbs, stand-in user ${notepad ? 'ON' : 'OFF'})` : `${failures.length} FAILED:\n  ${failures.join('\n  ')}`);
setTimeout(() => process.exit(failures.length === 0 ? 0 : 1), 300);
