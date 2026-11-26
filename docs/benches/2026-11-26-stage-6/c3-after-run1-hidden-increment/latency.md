exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 19.2 ms, p95 22.1 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 15.4 | 19.4 | 13.4 | 19.4 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 14.6 | 17.6 | 12.7 | 17.6 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 4.4 | 5.7 | 3.7 | 5.7 | 20/20 | 0 | 0 | 0 | 20 |
| act_double_click[increment] | 20 | 41.5 | 43.6 | 37.2 | 43.6 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.6 | 7.6 | 4.2 | 7.6 | 20/20 | 0 | 20 | 0 | 0 |
