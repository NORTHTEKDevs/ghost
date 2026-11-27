exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 19.1 ms, p95 37.0 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 15.1 | 20.2 | 10.0 | 20.2 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 13.5 | 17.8 | 11.0 | 17.8 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 4.1 | 5.6 | 3.3 | 5.6 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 35.0 | 47.9 | 31.6 | 47.9 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.0 | 7.8 | 3.5 | 7.8 | 20/20 | 0 | 20 | 0 | 0 |
