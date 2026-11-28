// node --test scripts/demo-launch.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { launchHidden } from './demo-launch.mjs';

// The launch replies ghost-mcp really sends (crates/ghost-session/src/hidden.rs
// and the foreground-policy path in crates/ghost-mcp/src/main.rs).
const TARGET = { hwnd: 4242, title: 'Invoice Editor', pid: 77, surface: 'user' };
const HIDDEN = { pid: 77, surface: 'hidden', desktop: 'auto', target: { ...TARGET, surface: 'hidden' } };
const REUSED = { pid: 77, surface: 'user', desktop: null, window: { hwnd: 4242, title: 'Invoice Editor', pid: 77 }, target: TARGET, warning: '...' };
const FOREGROUND = { pid: 77, surface: 'user' };

// A stand-in for demo-capture's `call` and a recorder for process kills.
function fakes(launch, { actFails = false } = {}) {
  const calls = [];
  const kills = [];
  const call = async (name, args) => {
    calls.push({ name, args });
    if (name === 'ghost_act' && actFails) throw new Error('quit failed');
    return { body: name === 'ghost_window' ? launch : { ok: true } };
  };
  const kill = (pid) => kills.push(pid);
  return { call, kill, calls, kills };
}

const ARGS = { op: 'launch', exe: '"testbed.exe" --title "Invoice Editor"' };

test('a launch on the hidden desktop is returned and nothing is closed', async () => {
  const f = fakes(HIDDEN);
  assert.equal((await launchHidden(f.call, ARGS, f.kill)).surface, 'hidden');
  assert.deepEqual(f.calls.map((c) => c.name), ['ghost_window']);
  assert.deepEqual(f.kills, []);
});

test('an anchored window on the user desktop is closed through its Quit button, then the launch fails', async () => {
  const f = fakes(REUSED);
  await assert.rejects(launchHidden(f.call, ARGS, f.kill), /did not start on a hidden desktop/);
  assert.deepEqual(f.calls.map((c) => c.name), ['ghost_window', 'ghost_act']);
  assert.deepEqual(f.calls[1].args, { name: 'Quit', action: 'click' });
  assert.deepEqual(f.kills, []);
});

test('a foreground launch with no anchor ends the process it started', async () => {
  const f = fakes(FOREGROUND);
  await assert.rejects(launchHidden(f.call, ARGS, f.kill), /did not start on a hidden desktop/);
  assert.deepEqual(f.calls.map((c) => c.name), ['ghost_window']);
  assert.deepEqual(f.kills, [77]);
});

test('a Quit that fails falls back to ending the process, and the launch error still surfaces', async () => {
  const f = fakes(REUSED, { actFails: true });
  await assert.rejects(launchHidden(f.call, ARGS, f.kill), /did not start on a hidden desktop/);
  assert.deepEqual(f.kills, [77]);
});
