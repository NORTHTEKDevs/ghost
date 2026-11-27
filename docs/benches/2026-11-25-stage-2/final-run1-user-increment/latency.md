exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 17.7 ms, p95 1198.4 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 29.1 | 35.1 | 26.6 | 35.1 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 30.8 | 33.4 | 27.7 | 33.4 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 11.8 | 14.3 | 9.5 | 14.3 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 43.2 | 43.9 | 41.7 | 43.9 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 4.3 | 5.3 | 3.6 | 5.3 | 20/20 | 0 | 20 | 0 | 0 |
