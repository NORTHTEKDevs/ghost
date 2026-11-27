exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 28.1 ms, p95 56.3 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 12.8 | 16.8 | 9.5 | 16.8 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 18.1 | 30.0 | 11.9 | 30.0 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 7.5 | 11.1 | 5.7 | 11.1 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 38.4 | 52.7 | 31.4 | 52.7 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 4.9 | 6.1 | 3.9 | 6.1 | 20/20 | 0 | 20 | 0 | 0 |
