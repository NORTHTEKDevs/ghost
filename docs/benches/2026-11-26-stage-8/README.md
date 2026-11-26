# Stage 8 - error kinds, C6, C7 / delta measurement, C5 / C8 measurement

Probe-safety rule as in stage 7: probes send input only to windows they create;
the cover probe checks `WindowFromPoint` before every click.

## 1. Refusals raised with the right error kind

The click_at cover refusal was `GhostError::Vision`, so agents read
"Vision error: ...". It is now `ElementNotInteractable` ("Element not
interactable: the point (x,y) in '<target>' - it is covered by '<cover>' (pid
N) ..."), the kind and code (-32002) a disabled element already gets, whose hint
says "disabled or covered". The same nit applied to errors added in stages 6-7,
all fixed here: a resolved window that has closed (`pick_window`, `read_text`,
`describe_window`) is `ProcessNotFound`, like C11's closed anchor; a resolved
window that is minimised is `ElementNotInteractable`. The pre-existing "no
visible window matching" message keeps its original kind.

| Criterion | Target | Measured |
|---|---|---|
| Covered click_at refusal starts "Element not interactable" (`err-cover`) | 20/20 | 20/20; uncovered and popup unchanged 20/20 |
| Unit: cover refusal kind and text; closed -> ProcessNotFound; minimised -> ElementNotInteractable | as stated | 2/2 (red at compile before) |
| `cargo test --workspace` | 0 failed | 606 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | 0 / 0 findings |
