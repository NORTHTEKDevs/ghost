# Ghost MCP benchmark results

- Tasks: **12/12 passed (100.0%)**
- Latency: median 2472.1 ms, max 3547.7 ms (full task incl. app launch)
- Run: 2026-07-02T09:41:49Z

| Task | Result | Latency | Detail |
|---|---|---|---|
| find_button | PASS | 2407 ms | center={'x': 90, 'y': 376} source=uia |
| click_compute | PASS | 3548 ms | display='Display is 42' |
| keyboard_compute | PASS | 2969 ms | display='Display is 81' |
| act_verified | PASS | 2429 ms | verified=True focus=True |
| wait_for_element | PASS | 1181 ms | appeared=True confirmed=True |
| window_list_state | PASS | 2277 ms | found=1 state=normal |
| read_text | PASS | 2743 ms | read_back='Display is 123' |
| index_disambiguation | PASS | 2371 ms | matches=26 name='Minimize Calculator' |
| run_chaining | PASS | 2516 ms | completed=8/8 display='Display is 13' |
| structured_error | PASS | 3069 ms | code=-32001 has_suggestion=True |
| screenshot_element | PASS | 2315 ms | size_bytes=1031 valid_image=True |
| value_equals_assert | PASS | 3322 ms | read_back='benchmark-value' assert_passed=True |
