# Ghost MCP benchmark results

- Tasks: **14/14 passed (100.0%)**
- Latency: median 2861.2 ms, max 7357.3 ms (full task incl. app launch)

| Task | Result | Latency | Detail |
|---|---|---|---|
| find_button | PASS | 2485 ms | center={'x': 90, 'y': 376} source=uia |
| click_compute | PASS | 7357 ms | display='Display is 42' |
| keyboard_compute | PASS | 3367 ms | display='Display is 81' |
| act_verified | PASS | 2425 ms | verified=True focus_preserved=True cursor_preserved=True |
| wait_for_element | PASS | 844 ms | appeared=True confirmed=True |
| window_list_state | PASS | 3008 ms | found=12 state=normal |
| window_minimize_restore | PASS | 3008 ms | minimized=True restored=True |
| read_text | PASS | 2714 ms | read_back='Display is 123' |
| index_disambiguation | PASS | 2287 ms | matches=36 name='Minimize Calculator' |
| run_chaining | PASS | 3239 ms | completed=8/8 display='Display is 13' |
| clipboard_roundtrip | PASS | 152 ms | got='ghost-bench-clip-7f3a' |
| structured_error | PASS | 3784 ms | code=-32001 has_suggestion=True |
| screenshot_element | PASS | 2313 ms | size_bytes=1068 valid_image=True |
| value_equals_assert | PASS | 3215 ms | read_back='benchmark-value' assert_passed=True |
