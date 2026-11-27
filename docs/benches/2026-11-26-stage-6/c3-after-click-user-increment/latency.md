exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 20.7 ms, p95 22.0 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 44.0 | 51.6 | 37.8 | 51.6 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 34.5 | 38.3 | 30.9 | 38.3 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 16.8 | 20.2 | 13.3 | 20.2 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 43.2 | 59.0 | 42.4 | 59.0 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 6.5 | 7.7 | 5.2 | 7.7 | 20/20 | 0 | 20 | 0 | 0 |
