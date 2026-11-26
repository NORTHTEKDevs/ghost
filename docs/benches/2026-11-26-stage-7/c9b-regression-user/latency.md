exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 17.8 ms, p95 56.8 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 19.5 | 27.0 | 14.6 | 27.0 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 23.2 | 29.5 | 12.9 | 29.5 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 17.6 | 20.5 | 12.7 | 20.5 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 43.2 | 60.1 | 41.9 | 60.1 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 4.7 | 6.4 | 3.9 | 6.4 | 20/20 | 0 | 20 | 0 | 0 |
