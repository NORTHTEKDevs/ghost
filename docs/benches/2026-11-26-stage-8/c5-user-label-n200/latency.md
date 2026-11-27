exe: <repo>\target-improve\release\ghost-mcp.exe
surface: user
control: label

cold start (spawn->initialize, n=10): median 19.3 ms, p95 60.4 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| act_click[label] | 200 | 137.7 | 142.8 | 116.6 | 147.0 | 200/200 | 0 | 0 | 200 | 0 |
