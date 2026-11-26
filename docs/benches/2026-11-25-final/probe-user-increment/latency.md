exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 19.0 ms, p95 20.6 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 34.0 | 39.2 | 28.7 | 39.2 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 35.0 | 39.0 | 29.8 | 39.0 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 12.8 | 15.1 | 9.6 | 15.1 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 43.2 | 57.1 | 41.5 | 57.1 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.8 | 7.3 | 5.0 | 7.3 | 20/20 | 0 | 20 | 0 | 0 |
