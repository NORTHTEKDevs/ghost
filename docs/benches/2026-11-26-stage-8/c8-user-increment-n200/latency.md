exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: increment

cold start (spawn->initialize, n=10): median 18.8 ms, p95 58.7 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| see_text | 200 | 21.5 | 26.9 | 13.2 | 31.9 | 200/200 | 0 | 0 | 0 | 200 |
| see_full | 200 | 20.1 | 29.3 | 13.9 | 33.2 | 200/200 | 0 | 0 | 0 | 200 |
| find | 200 | 16.5 | 22.4 | 11.4 | 27.1 | 200/200 | 0 | 0 | 0 | 200 |
| act_click[increment] | 200 | 57.9 | 60.0 | 41.2 | 73.3 | 200/200 | 0 | 200 | 0 | 0 |
| act_type | 200 | 6.0 | 8.3 | 3.9 | 12.4 | 200/200 | 0 | 200 | 0 | 0 |
