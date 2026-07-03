# Ghost MCP benchmark results

- Tasks: **14/14 passed (100.0%)**
- Latency: median 2716.6 ms, max 3520.4 ms (full task incl. app launch)
- Run: 2026-07-03T23:34:42Z

| Task | Result | Latency | Detail |
|---|---|---|---|
| find_button | PASS | 2444 ms | center={'x': 90, 'y': 376} source=uia |
| click_compute | PASS | 3510 ms | display='Display is 42' |
| keyboard_compute | PASS | 3046 ms | display='Display is 81' |
| act_verified | PASS | 2935 ms | verified=True focus=True |
| wait_for_element | PASS | 1490 ms | appeared=True confirmed=True |
| window_list_state | PASS | 2498 ms | found=1 state=normal |
| window_minimize_restore | PASS | 3301 ms | minimized=True restored=True |
| read_text | PASS | 3098 ms | read_back='Display is 123' |
| index_disambiguation | PASS | 2489 ms | matches=26 name='Minimize Calculator' |
| run_chaining | PASS | 2465 ms | completed=8/8 display='Display is 13' |
| clipboard_roundtrip | PASS | 161 ms | got='ghost-bench-clip-7f3a' |
| structured_error | PASS | 3520 ms | code=-32001 has_suggestion=True |
| screenshot_element | PASS | 2378 ms | size_bytes=1031 valid_image=True |
| value_equals_assert | PASS | 3330 ms | read_back='benchmark-value' assert_passed=True |
