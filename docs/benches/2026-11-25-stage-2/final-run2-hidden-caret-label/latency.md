exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden
control: caret-label

cold start (spawn->initialize, n=10): median 17.0 ms, p95 20.9 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| act_click[caret-label] | 20 | 120.8 | 129.3 | 108.5 | 129.3 | 20/20 | 0 | 0 | 20 | 0 |
