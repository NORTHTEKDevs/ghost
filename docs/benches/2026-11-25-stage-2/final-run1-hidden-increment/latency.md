exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 16.0 ms, p95 18.6 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 18.2 | 21.8 | 12.7 | 21.8 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 15.4 | 20.4 | 10.5 | 20.4 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 3.5 | 4.3 | 3.2 | 4.3 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 35.3 | 47.7 | 32.1 | 47.7 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.3 | 7.0 | 3.9 | 7.0 | 20/20 | 0 | 20 | 0 | 0 |
