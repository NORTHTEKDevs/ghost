exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: label

cold start (spawn->initialize, n=10): median 16.1 ms, p95 19.8 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| act_click[label] | 20 | 123.2 | 133.9 | 117.1 | 133.9 | 20/20 | 0 | 0 | 20 | 0 |
