exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 18.1 ms, p95 53.3 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 14.2 | 16.4 | 11.3 | 16.4 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 33.4 | 40.4 | 28.7 | 40.4 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 13.4 | 18.2 | 10.6 | 18.2 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 42.9 | 44.0 | 41.0 | 44.0 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 6.0 | 9.2 | 4.4 | 9.2 | 20/20 | 0 | 20 | 0 | 0 |
