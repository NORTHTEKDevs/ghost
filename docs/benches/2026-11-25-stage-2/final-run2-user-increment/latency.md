exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 15.6 ms, p95 18.9 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 29.5 | 38.1 | 24.9 | 38.1 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 28.6 | 31.9 | 27.2 | 31.9 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 12.4 | 17.1 | 9.4 | 17.1 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 43.3 | 49.3 | 34.4 | 49.3 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 4.1 | 6.0 | 3.6 | 6.0 | 20/20 | 0 | 20 | 0 | 0 |
