exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 23.1 ms, p95 1475.4 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 35.4 | 44.7 | 31.2 | 44.7 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 39.7 | 44.0 | 34.7 | 44.0 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 12.6 | 17.8 | 11.1 | 17.8 | 20/20 | 0 | 0 | 0 | 20 |
| act_double_click[increment] | 20 | 43.5 | 59.1 | 42.3 | 59.1 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.5 | 7.2 | 4.7 | 7.2 | 20/20 | 0 | 20 | 0 | 0 |
