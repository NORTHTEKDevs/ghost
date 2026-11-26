exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 19.7 ms, p95 22.6 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 20.7 | 26.3 | 16.7 | 26.3 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 16.5 | 20.3 | 12.8 | 20.3 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 4.1 | 5.4 | 3.0 | 5.4 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 38.6 | 47.6 | 32.1 | 47.6 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.2 | 7.2 | 3.6 | 7.2 | 20/20 | 0 | 20 | 0 | 0 |
