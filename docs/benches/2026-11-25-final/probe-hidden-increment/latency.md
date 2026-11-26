exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 19.6 ms, p95 23.6 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 15.4 | 20.1 | 12.9 | 20.1 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 15.7 | 17.5 | 14.1 | 17.5 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 5.2 | 6.9 | 3.8 | 6.9 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 41.9 | 53.8 | 31.9 | 53.8 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.3 | 7.7 | 4.4 | 7.7 | 20/20 | 0 | 20 | 0 | 0 |
