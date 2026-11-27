exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden

cold start (spawn->initialize, n=10): median 20.7 ms, p95 41.3 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=null |
|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 18.0 | 25.3 | 10.8 | 25.3 | 20/20 | 0 | 0 | 20 |
| see_full | 20 | 14.2 | 23.6 | 9.2 | 23.6 | 20/20 | 0 | 0 | 20 |
| find | 20 | 10.0 | 15.1 | 6.2 | 15.1 | 20/20 | 0 | 0 | 20 |
| act_click | 20 | 121.4 | 128.3 | 101.9 | 128.3 | 20/20 | 0 | 10 | 0 |
| act_type | 20 | 8.1 | 10.8 | 6.0 | 10.8 | 20/20 | 0 | 20 | 0 |
