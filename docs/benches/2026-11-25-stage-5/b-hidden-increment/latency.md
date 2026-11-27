exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 18.5 ms, p95 21.1 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 9.5 | 13.0 | 8.1 | 13.0 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 10.8 | 12.6 | 9.2 | 12.6 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 4.1 | 7.5 | 3.4 | 7.5 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 33.6 | 38.9 | 31.0 | 38.9 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 4.2 | 5.3 | 3.6 | 5.3 | 20/20 | 0 | 20 | 0 | 0 |
