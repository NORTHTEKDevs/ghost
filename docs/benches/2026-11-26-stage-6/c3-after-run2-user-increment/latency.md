exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 18.3 ms, p95 20.4 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 37.6 | 50.7 | 31.3 | 50.7 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 45.4 | 51.2 | 35.5 | 51.2 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 15.9 | 21.5 | 12.9 | 21.5 | 20/20 | 0 | 0 | 0 | 20 |
| act_double_click[increment] | 20 | 44.1 | 59.5 | 41.8 | 59.5 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 6.3 | 7.7 | 5.0 | 7.7 | 20/20 | 0 | 20 | 0 | 0 |
