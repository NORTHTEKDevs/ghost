// The launch step of scripts/demo-capture.mjs, separate so it can be tested
// without a running ghost-mcp (scripts/demo-launch.test.mjs).

/**
 * Launch the testbed and insist it landed on Ghost's hidden desktop. If it
 * landed anywhere else, end the testbed this launch started before failing,
 * so a failed run never leaves it on the user's screen:
 * - a reply with `target` means Ghost anchored the testbed's window, so it is
 *   closed through that anchor (its own Quit button), never a title search
 *   that could match one of the user's windows;
 * - otherwise, or if Quit fails, the process the launch reported (`pid`) is
 *   ended. The script's own ghost-mcp started it, so it is ours to end.
 * @param {(name: string, args: object) => Promise<{body: any}>} call
 * @param {object} args ghost_window op=launch arguments
 * @param {(pid: number) => void} [kill] ends a process (default process.kill)
 * @returns {Promise<any>} the launch answer
 */
export async function launchHidden(call, args, kill = (pid) => process.kill(pid)) {
  const { body } = await call('ghost_window', args);
  if (body?.surface === 'hidden') return body;
  let closed = false;
  if (body?.target) {
    closed = await call('ghost_act', { name: 'Quit', action: 'click' }).then(
      () => true,
      (e) => { console.error(`Quit failed: ${e.message}`); return false; },
    );
  }
  if (!closed && Number.isInteger(body?.pid) && body.pid > 0) {
    try { kill(body.pid); } catch (e) { console.error(`ending pid ${body.pid} failed: ${e.message}`); }
  }
  throw new Error(`testbed did not start on a hidden desktop: ${JSON.stringify(body)}`);
}
