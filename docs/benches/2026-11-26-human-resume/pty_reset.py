# Run `ghost reset` in a real pseudo-console (ConPTY via pywinpty), read the
# challenge word it shows, type it back, and print the transcript.
# Usage: python pty_reset.py <ghost.exe>   (env carries GHOST_ISOLATED_STOP_NS)
import os
import re
import sys
import time

from winpty import PtyProcess

exe = sys.argv[1]
p = PtyProcess.spawn([exe, "reset"], env=dict(os.environ))
out = ""
deadline = time.time() + 15
word = None
while time.time() < deadline and p.isalive():
    try:
        out += p.read(4096)
    except EOFError:
        break
    m = re.search(r"Type ([a-z]{5}) to lift", out)
    if m and word is None:
        word = m.group(1)
        p.write(word + "\r\n")
    if "lifted" in out or "no emergency stop" in out or "not " + (word or "~") in out:
        time.sleep(0.3)
        try:
            out += p.read(4096)
        except EOFError:
            pass
        break
code = p.wait() if not p.isalive() else None
clean = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07", "", out)
print(f"[pty] challenge word: {word}")
print("[pty] transcript: " + " ".join(clean.split()))
print(f"[pty] exit code: {code}")
