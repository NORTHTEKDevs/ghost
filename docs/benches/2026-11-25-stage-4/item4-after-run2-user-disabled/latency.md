exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: disabled

cold start (spawn->initialize, n=10): median 20.6 ms, p95 54.2 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| act_click[disabled] | 20 | 20.1 | 26.8 | 10.5 | 26.8 | 20/20 | 20 | 0 | 0 | 20 |
