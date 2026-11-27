exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 16.2 ms, p95 52.3 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 14.1 | 18.7 | 10.6 | 18.7 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 34.0 | 45.8 | 27.8 | 45.8 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 13.2 | 17.4 | 10.2 | 17.4 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 43.2 | 45.4 | 41.7 | 45.4 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 4.6 | 7.0 | 4.1 | 7.0 | 20/20 | 0 | 20 | 0 | 0 |
