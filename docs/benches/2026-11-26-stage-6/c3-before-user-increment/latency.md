exe: C:\Users\Krist\projects\active\ghost\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 18.7 ms, p95 50.7 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 20 | 32.7 | 37.3 | 30.2 | 37.3 | 20/20 | 0 | 0 | 0 | 20 |
| see_full | 20 | 36.6 | 57.0 | 30.7 | 57.0 | 20/20 | 0 | 0 | 0 | 20 |
| find | 20 | 11.5 | 17.7 | 10.1 | 17.7 | 20/20 | 0 | 0 | 0 | 20 |
| act_double_click[increment] | 20 | 123.1 | 137.5 | 117.5 | 137.5 | 20/20 | 0 | 0 | 20 | 0 |
| act_type | 20 | 5.4 | 8.4 | 4.4 | 8.4 | 20/20 | 0 | 20 | 0 | 0 |
