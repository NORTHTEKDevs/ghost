exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 18.8 ms, p95 59.4 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 19.8 | 30.9 | 12.9 | 30.9 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 21.1 | 29.0 | 13.7 | 29.0 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 12.3 | 16.0 | 10.5 | 16.0 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 42.6 | 58.6 | 41.9 | 58.6 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 4.4 | 6.1 | 3.7 | 6.1 | 20/20 | 0 | 20 | 0 | 0 |
