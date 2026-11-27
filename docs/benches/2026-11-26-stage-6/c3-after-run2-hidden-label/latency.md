exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden
control: label

cold start (spawn->initialize, n=10): median 18.8 ms, p95 50.8 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| act_double_click[label] | 20 | 118.4 | 131.1 | 112.5 | 131.1 | 20/20 | 0 | 0 | 20 | 0 |
