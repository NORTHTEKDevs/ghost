exe: <repo>\target-improve\release\ghost-mcp.exe
surface: hidden
control: caret-label

cold start (spawn->initialize, n=10): median 38.8 ms, p95 61.1 ms
tools/list: 54 tools, 32094 bytes

| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |
|---|---|---|---|---|---|---|---|---|---|---|
| act_click[caret-label] | 20 | 122.1 | 139.8 | 112.9 | 139.8 | 20/20 | 0 | 0 | 20 | 0 |
