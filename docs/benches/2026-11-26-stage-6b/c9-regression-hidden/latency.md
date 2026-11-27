exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 19.1 ms, p95 54.6 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 17.0 | 23.8 | 13.9 | 23.8 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 16.4 | 20.2 | 12.5 | 20.2 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 4.9 | 6.2 | 3.9 | 6.2 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 37.5 | 48.5 | 32.8 | 48.5 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 4.7 | 5.8 | 3.8 | 5.8 | 20/20 | 0 | 20 | 0 | 0 |
