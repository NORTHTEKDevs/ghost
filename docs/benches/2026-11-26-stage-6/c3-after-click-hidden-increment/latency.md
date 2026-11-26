exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: hidden
control: increment

cold start (spawn->initialize, n=10): median 18.6 ms, p95 21.9 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 16.3 | 22.8 | 11.1 | 22.8 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 18.0 | 21.5 | 15.0 | 21.5 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 5.4 | 6.3 | 4.4 | 6.3 | 20/20 | 0 | 0 | 0 | 20 |
| act_click[increment] | 20 | 42.6 | 48.9 | 36.7 | 48.9 | 20/20 | 0 | 20 | 0 | 0 |
| act_type | 20 | 5.5 | 8.0 | 4.7 | 8.0 | 20/20 | 0 | 20 | 0 | 0 |
