exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 17.9 ms, p95 19.9 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 33.5 | 36.9 | 29.1 | 36.9 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 34.0 | 40.6 | 30.7 | 40.6 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 12.0 | 14.6 | 9.9 | 14.6 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 50.4 | 60.6 | 41.9 | 60.6 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 6.3 | 11.1 | 4.3 | 11.1 | 20/20 | 0 | 20 | 0 | 0 |
