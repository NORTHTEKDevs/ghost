"""Minimal Ghost client shared by the recipes.

Speaks stdio JSON-RPC to ghost-mcp.exe. One process, serial calls - which is
what you want for automation: no interleaving, no surprises about ordering.
"""
import json
import os
import shutil
import subprocess
import sys


def find_ghost_mcp():
    """Locate ghost-mcp.exe: next to the recipes, on PATH, or in a source build."""
    here = os.path.dirname(os.path.abspath(__file__))
    for candidate in (
        os.path.join(here, "..", "ghost-mcp.exe"),          # unzipped kit
        os.path.join(here, "..", "..", "ghost-mcp.exe"),    # installed layout
        os.path.join(here, "..", "target", "release", "ghost-mcp.exe"),
        os.path.join(here, "..", "..", "target", "release", "ghost-mcp.exe"),
    ):
        if os.path.isfile(candidate):
            return os.path.abspath(candidate)
    on_path = shutil.which("ghost-mcp")
    if on_path:
        return on_path
    sys.exit(
        "Could not find ghost-mcp.exe.\n"
        "Run install.ps1 first, or run these recipes from the folder you unzipped."
    )


class GhostError(RuntimeError):
    pass


class Ghost:
    def __init__(self, exe=None):
        self.exe = exe or find_ghost_mcp()
        self.p = subprocess.Popen(
            [self.exe],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, encoding="utf-8", bufsize=1,
        )
        self._id = 0
        self._rpc("initialize", {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "ghost-recipe", "version": "1"},
        })

    def _rpc(self, method, params):
        self._id += 1
        self.p.stdin.write(json.dumps(
            {"jsonrpc": "2.0", "id": self._id, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        if not line:
            raise GhostError("ghost-mcp closed the connection")
        return json.loads(line)

    def call(self, tool, **params):
        """Invoke a Ghost tool. Raises on failure rather than returning something falsy.

        Ghost reports problems TWO ways: as a JSON-RPC error, and as a payload
        with `ok: false` and an `error` string. Checking only the first lets a
        failed call look like an empty success - which is how automation ends up
        silently doing nothing. Both are checked here.

        Returns the unwrapped `data` when the payload uses that envelope, so
        callers work with the useful part directly.
        """
        resp = self._rpc("tools/call", {"name": tool, "arguments": params})
        if "error" in resp:
            raise GhostError(f"{tool}: {resp['error'].get('message', resp['error'])}")

        payload = resp.get("result", {})
        content = payload.get("content", [])
        if content and content[0].get("type") == "text":
            text = content[0]["text"]
            try:
                payload = json.loads(text)
            except json.JSONDecodeError:
                return {"text": text}

        if isinstance(payload, dict):
            if payload.get("ok") is False or payload.get("error"):
                detail = payload.get("error") or "call failed"
                hint = payload.get("suggested_action")
                raise GhostError(f"{tool}: {detail}" + (f" (try: {hint})" if hint else ""))
            # Unwrap the {ok, data, ms, ...} envelope when present.
            if "data" in payload and set(payload) & {"ok", "ms", "foreground"}:
                return payload["data"]
        return payload

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
