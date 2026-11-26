exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 18.6 ms, p95 20.7 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 16.7 | 20.6 | 13.7 | 20.6 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 17.1 | 23.8 | 13.8 | 23.8 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 6.6 | 10.6 | 4.3 | 10.6 | 20/20 | 0 | 0 | 0 | 20 |
| act_double_click[increment] | 20 | 38.1 | 47.5 | 32.3 | 47.5 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.1 | 6.2 | 4.1 | 6.2 | 20/20 | 0 | 20 | 0 | 0 |
