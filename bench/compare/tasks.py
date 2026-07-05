"""Cross-tool computer-use task set.

These are the capabilities a computer-use agent tool should have. Each task is
tool-agnostic (any tool that can drive a Windows desktop could attempt it) and
scored by RE-OBSERVING the real result, never by trusting a tool's return.

An adapter (see adapters.py) implements how a given tool attempts each task.
Ghost has a working adapter; competitors are pluggable (bring the tool).
"""

# Each task: id, title, category, and why it matters / which tools it separates.
TASKS = [
    {
        "id": "compute",
        "title": "Launch Calculator, compute 6 x 7, read the display = 42",
        "category": "perceive + act + verify",
        "matters": "the basic loop: launch, act, and confirm by reading the real result",
    },
    {
        "id": "type_readback",
        "title": "Type text into a native field and read it back exactly",
        "category": "act + verify",
        "matters": "did the text actually land — verified by reading the value, not a return code",
    },
    {
        "id": "no_api_native",
        "title": "Operate a native Win32 app with no API/CDP (Character Map)",
        "category": "no-API reach",
        "matters": "the software with no integration — where browser tools (Playwright) can't go",
    },
    {
        "id": "background_no_focus_steal",
        "title": "Act inside an app WITHOUT taking foreground or moving the cursor",
        "category": "background dispatch (differentiator)",
        "matters": "run an agent while the human keeps working; most tools can only drive the foreground",
    },
    {
        "id": "per_action_verified",
        "title": "Report per-action whether the effect actually happened",
        "category": "verification (differentiator)",
        "matters": "closes the #1 agent failure: acting blind and not knowing if it worked",
    },
    {
        "id": "structured_read",
        "title": "Read the UI as structured data (elements/text), not just a screenshot",
        "category": "cheap perception",
        "matters": "let an agent plan over structure — far cheaper in tokens than images",
    },
]
