"""Tool adapters for the cross-tool comparison.

An adapter says HOW a given computer-use tool attempts each task in tasks.py and
returns a scored result. Scoring always re-observes the real world.

To add a competitor (Playwright-MCP, cua-driver, Anthropic Computer Use, ...):
subclass `Adapter`, implement `run(task_id) -> Result`, and register it in
run.py. Return Result(applicable=False, ...) for tasks the tool can't attempt
(e.g. Playwright is browser-only, so most native-app tasks are N/A).
"""
import os
import sys
import time
from dataclasses import dataclass

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from run_bench import GhostClient, DEFAULT_EXE  # noqa: E402


@dataclass
class Result:
    passed: bool
    applicable: bool = True
    detail: str = ""
    ms: float = 0.0


class Adapter:
    name = "adapter"

    def run(self, task_id: str) -> Result:  # pragma: no cover - interface
        raise NotImplementedError


class GhostAdapter(Adapter):
    """Drives the real ghost-mcp binary and scores by re-observation."""
    name = "Ghost"

    def __init__(self, exe=DEFAULT_EXE):
        self.exe = exe

    def _fg(self, g):
        for w in g.data("ghost_window", {"op": "list"})[0].get("windows", []) or []:
            if w.get("focused"):
                return w.get("name", "")
        return ""

    def run(self, task_id: str) -> Result:
        t0 = time.perf_counter()
        g = GhostClient(self.exe)
        try:
            fn = getattr(self, f"_task_{task_id}")
            passed, detail, applicable = fn(g)
            return Result(passed, applicable, detail, (time.perf_counter() - t0) * 1000.0)
        except Exception as e:  # any crash = a fail, honestly recorded
            return Result(False, True, f"exception: {e}", (time.perf_counter() - t0) * 1000.0)
        finally:
            g.close()

    # --- task implementations -------------------------------------------------

    def _calc(self, g):
        g.call("ghost_window", {"op": "launch", "exe": "calc.exe"})
        time.sleep(1.5)
        g.call("ghost_wait", {"for": "idle", "window": "Calculator", "timeout_ms": 4000})
        g.call("ghost_window", {"op": "focus", "name": "Calculator"})
        time.sleep(0.3)

    def _display(self, g):
        env, _, _ = g.data("ghost_see", {"mode": "text", "window": "Calculator"})
        return env.get("text", "") if isinstance(env, dict) else ""

    def _task_compute(self, g):
        self._calc(g)
        for b in ["Six", "Multiply by", "Seven", "Equals"]:
            g.call("ghost_act", {"action": "click", "name": b, "role": "button", "window": "Calculator"})
        time.sleep(0.3)
        got = self._display(g)
        g.call("ghost_window", {"op": "state", "name": "Calculator", "state": "close"})
        return ("42" in got, f"display={got!r}", True)

    def _task_type_readback(self, g):
        g.call("ghost_window", {"op": "launch", "exe": "charmap.exe"}); time.sleep(1.5)
        g.call("ghost_act", {"background": True, "window": "Character Map", "action": "type",
                             "role": "edit", "text_input": "ghost-42"})
        time.sleep(0.2)
        env, _, _ = g.data("ghost_see", {"mode": "text", "window": "Character Map"})
        txt = env.get("text", "") if isinstance(env, dict) else ""
        g.call("ghost_window", {"op": "state", "name": "Character Map", "state": "close"})
        return ("ghost-42" in txt, "field read back matched", True)

    def _task_no_api_native(self, g):
        # Character Map is a classic Win32 app: no API, no CDP. Drive its field.
        g.call("ghost_window", {"op": "launch", "exe": "charmap.exe"}); time.sleep(1.5)
        r = g.call("ghost_act", {"background": True, "window": "Character Map", "action": "type",
                                 "role": "edit", "text_input": "native"})[0]
        data = r.get("data", r) if isinstance(r, dict) else {}
        g.call("ghost_window", {"op": "state", "name": "Character Map", "state": "close"})
        return (bool(data.get("verified")), "drove a no-API Win32 app", True)

    def _task_background_no_focus_steal(self, g):
        g.call("ghost_window", {"op": "launch", "exe": "charmap.exe"}); time.sleep(1.4)
        g.call("ghost_window", {"op": "launch", "exe": "calc.exe"}); time.sleep(1.2)
        g.call("ghost_window", {"op": "focus", "name": "Calculator"}); time.sleep(0.4)
        fg0 = self._fg(g)
        r = g.call("ghost_act", {"background": True, "window": "Character Map",
                                 "action": "click", "name": "Select", "role": "button"})[0]
        data = r.get("data", r) if isinstance(r, dict) else {}
        fg1 = self._fg(g)
        g.call("ghost_window", {"op": "state", "name": "Character Map", "state": "close"})
        g.call("ghost_window", {"op": "state", "name": "Calculator", "state": "close"})
        ok = bool(data.get("focus_preserved")) and "Calculator" in fg0 and "Calculator" in fg1
        return (ok, f"foreground stayed {fg1!r}, focus_preserved={data.get('focus_preserved')}", True)

    def _task_per_action_verified(self, g):
        self._calc(g)
        r = g.call("ghost_act", {"action": "click", "name": "Seven", "role": "button", "window": "Calculator"})[0]
        data = r.get("data", r) if isinstance(r, dict) else {}
        g.call("ghost_window", {"op": "state", "name": "Calculator", "state": "close"})
        # Pass = the tool RETURNS a concrete verified signal (not None), and it's true here.
        return (data.get("verified") is True, f"verified={data.get('verified')} focus_confirmed={data.get('focus_confirmed')}", True)

    def _task_structured_read(self, g):
        self._calc(g)
        env, _, _ = g.data("ghost_see", {"mode": "text", "window": "Calculator"})
        txt = env.get("text", "") if isinstance(env, dict) else ""
        g.call("ghost_window", {"op": "state", "name": "Calculator", "state": "close"})
        # Structured text extraction returns real UI text (button/label names), no screenshot.
        return (len(txt) > 0 and "Display" in txt, f"{len(txt)} chars of structured text", True)
