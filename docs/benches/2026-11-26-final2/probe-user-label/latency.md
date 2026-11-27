exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: label

cold start (spawn->initialize, n=10): median 17.5 ms, p95 59.7 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| act_click[label] | 20 | 123.4 | 138.7 | 117.1 | 138.7 | 20/20 | 0 | 0 | 20 | 0 |
