exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user

cold start (spawn->initialize, n=10): median 21.0 ms, p95 39.9 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=null |
|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 51.6 | 58.7 | 42.8 | 58.7 | 20/20 | 0 | 0 | 20 |
| see_full | 20 | 47.0 | 70.7 | 32.6 | 70.7 | 20/20 | 0 | 0 | 20 |
| find | 20 | 19.7 | 24.6 | 13.3 | 24.6 | 20/20 | 0 | 0 | 20 |
| act_click | 20 | 123.4 | 143.3 | 117.4 | 143.3 | 20/20 | 0 | 0 | 0 |
| act_type | 20 | 7.8 | 12.0 | 5.7 | 12.0 | 20/20 | 0 | 20 | 0 |
