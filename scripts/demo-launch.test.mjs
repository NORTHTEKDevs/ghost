// node --test scripts/demo-launch.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { launchHidden } from './demo-launch.mjs';

// A stand-in for demo-capture's `call`: answers the launch with `launch` and
// records every call made.
function fakeCall(launch) {
  const calls = [];
  const call = async (name, args) => {
    calls.push({ name, args });
    return { body: name === 'ghost_window' ? launch : { ok: true } };
  };
  return { call, calls };
}

const ARGS = { op: 'launch', exe: '"testbed.exe" --title "Invoice Editor"' };

test('a launch that lands on the hidden desktop is returned and nothing is closed', async () => {
  const { call, calls } = fakeCall({ surface: 'hidden', anchored: 'Invoice Editor' });
  const body = await launchHidden(call, ARGS);
  assert.equal(body.surface, 'hidden');
  assert.deepEqual(calls.map((c) => c.name), ['ghost_window']);
});

test('a launch that lands anywhere else closes the testbed it started, then fails', async () => {
  const { call, calls } = fakeCall({ surface: 'user', anchored: 'Invoice Editor' });
  await assert.rejects(launchHidden(call, ARGS), /did not start on a hidden desktop/);
  assert.deepEqual(calls.map((c) => c.name), ['ghost_window', 'ghost_act']);
  assert.deepEqual(calls[1].args, { name: 'Quit', action: 'click' });
});

test('a launch that anchored nothing closes nothing (no window to close by anchor)', async () => {
  const { call, calls } = fakeCall({ surface: 'user' });
  await assert.rejects(launchHidden(call, ARGS), /did not start on a hidden desktop/);
  assert.deepEqual(calls.map((c) => c.name), ['ghost_window']);
});

test('a failed close still reports the launch failure', async () => {
  const calls = [];
  const call = async (name) => {
    calls.push(name);
    if (name === 'ghost_act') throw new Error('quit failed');
    return { body: { surface: 'user', anchored: 'Invoice Editor' } };
  };
  await assert.rejects(launchHidden(call, ARGS), /did not start on a hidden desktop/);
  assert.deepEqual(calls, ['ghost_window', 'ghost_act']);
});
