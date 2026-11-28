// The launch step of scripts/demo-capture.mjs, separate so it can be tested
// without a running ghost-mcp (scripts/demo-launch.test.mjs).

/**
 * Launch the testbed and insist it landed on Ghost's hidden desktop. If it
 * landed anywhere else, close the testbed this launch started before failing,
 * so a failed run never leaves it on the user's screen. The close goes
 * through the anchor the launch set (the testbed's own Quit button), never a
 * title search, which could match one of the user's windows.
 * @param {(name: string, args: object) => Promise<{body: any}>} call
 * @param {object} args ghost_window op=launch arguments
 * @returns {Promise<any>} the launch answer
 */
export async function launchHidden(call, args) {
  const { body } = await call('ghost_window', args);
  if (body?.surface === 'hidden') return body;
  if (body?.anchored) {
    await call('ghost_act', { name: 'Quit', action: 'click' })
      .catch((e) => console.error(`closing the testbed failed: ${e.message}`));
  }
  throw new Error(`testbed did not start on a hidden desktop: ${JSON.stringify(body)}`);
}
