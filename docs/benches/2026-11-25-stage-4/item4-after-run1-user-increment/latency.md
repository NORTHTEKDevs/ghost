exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 19.3 ms, p95 21.6 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 33.6 | 37.4 | 29.5 | 37.4 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 32.8 | 40.7 | 29.4 | 40.7 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 11.9 | 14.7 | 9.7 | 14.7 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 44.2 | 59.7 | 41.5 | 59.7 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.3 | 6.8 | 4.3 | 6.8 | 20/20 | 0 | 20 | 0 | 0 |
