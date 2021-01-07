# MachList

tool for managing list of machines and doing simple things with it.

## Configuration

check for resources.toml


```toml
username = "env:USERNAME"

[server]

[server.env1.proxy]
ip = "1.2.3.4"
proxy = true

[server.env1.dest]
jump = "proxy"
name = "dest"

```

## Subcommands

* ssh machine
* list
