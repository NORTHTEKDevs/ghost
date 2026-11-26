exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 20.0 ms, p95 49.6 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 37.6 | 47.7 | 30.8 | 47.7 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 33.0 | 39.3 | 30.0 | 39.3 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 10.9 | 14.3 | 9.5 | 14.3 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 43.4 | 59.5 | 42.0 | 59.5 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.3 | 5.8 | 4.4 | 5.8 | 20/20 | 0 | 20 | 0 | 0 |
