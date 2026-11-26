exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 18.7 ms, p95 22.4 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 38.0 | 50.0 | 31.9 | 50.0 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 36.7 | 43.4 | 32.2 | 43.4 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 14.3 | 19.3 | 11.7 | 19.3 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 43.6 | 60.3 | 41.8 | 60.3 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.9 | 7.5 | 4.8 | 7.5 | 20/20 | 0 | 20 | 0 | 0 |
