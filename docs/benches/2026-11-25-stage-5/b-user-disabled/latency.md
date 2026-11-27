exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: disabled

cold start (spawn->initialize, n=10): median 19.4 ms, p95 1114.8 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| act_click[disabled] | 20 | 13.2 | 18.0 | 11.3 | 18.0 | 20/20 | 20 | 0 | 0 | 20 |
