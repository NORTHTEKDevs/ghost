exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user

cold start (spawn->initialize, n=10): median 17.5 ms, p95 31.3 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=null |
|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 30.8 | 39.4 | 27.6 | 39.4 | 20/20 | 0 | 0 | 20 |
| see_full | 20 | 29.0 | 36.3 | 26.0 | 36.3 | 20/20 | 0 | 0 | 20 |
| find | 20 | 10.4 | 13.0 | 8.2 | 13.0 | 20/20 | 0 | 0 | 20 |
| act_click | 20 | 122.3 | 127.6 | 107.1 | 127.6 | 20/20 | 0 | 0 | 0 |
| act_type | 20 | 4.8 | 6.3 | 3.6 | 6.3 | 20/20 | 0 | 20 | 0 |
