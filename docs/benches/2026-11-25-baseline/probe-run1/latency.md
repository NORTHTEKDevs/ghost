exe: <repo>\target-improve\release\ghost-mcp.exe

cold start (spawn->initialize, n=10): median 18.7 ms, p95 34.8 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=null |
|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 16.4 | 25.1 | 13.8 | 25.1 | 20/20 | 0 | 0 | 20 |
| see_full | 20 | 22.0 | 27.5 | 17.9 | 27.5 | 20/20 | 0 | 0 | 20 |
| find | 20 | 7.9 | 10.7 | 6.0 | 10.7 | 20/20 | 0 | 0 | 20 |
| act_click | 20 | 113.4 | 122.8 | 106.6 | 122.8 | 20/20 | 0 | 10 | 0 |
| act_type | 20 | 7.1 | 8.5 | 5.3 | 8.5 | 20/20 | 0 | 20 | 0 |
