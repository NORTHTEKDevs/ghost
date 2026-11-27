exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 18.5 ms, p95 23.9 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 12.5 | 16.1 | 10.9 | 16.1 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 13.6 | 18.8 | 11.9 | 18.8 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 5.1 | 7.8 | 4.1 | 7.8 | 20/20 | 0 | 0 | 0 | 20 |
| act_double_click[increment] | 20 | 115.2 | 128.9 | 111.9 | 128.9 | 20/20 | 0 | 5 | 15 | 0 |
| act_type | 20 | 5.2 | 8.1 | 4.1 | 8.1 | 20/20 | 0 | 20 | 0 | 0 |
