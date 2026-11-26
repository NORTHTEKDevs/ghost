exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 16.9 ms, p95 20.8 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 17.3 | 29.7 | 12.7 | 29.7 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 17.5 | 19.2 | 13.0 | 19.2 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 5.0 | 5.9 | 3.6 | 5.9 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 38.8 | 45.3 | 35.0 | 45.3 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 4.7 | 7.0 | 3.4 | 7.0 | 20/20 | 0 | 20 | 0 | 0 |
