# Memory sizing and OOM diagnosis

[Operations](OPERATIONS.md) · [State and limits](BEHAVIOR.md#state-and-limits)

Linux `Out of memory: Killed process` confirms memory pressure, not a leak. Confirm the executable
and service: `mini-sub2api-co` may be the truncated Rust Core name. `anon-rss` is resident anonymous
memory; `total-vm` is virtual address space. Budget for both Go and Rust, the OS and other services.

## Inspect before changing limits

Use the incident's boot/time window and actual service name. These commands are read-only:

```bash
free -h
swapon --show
ps -eo pid,ppid,comm,rss --sort=-rss | head -n 16
systemctl show mini-sub2api.service \
  -p MainPID -p ControlGroup -p MemoryCurrent -p MemoryPeak \
  -p MemoryHigh -p MemoryMax -p MemorySwapMax -p NRestarts -p Result
sudo journalctl -k --no-pager | rg -C 25 'oom-kill|Out of memory|Killed process'
sudo journalctl -u mini-sub2api.service -n 100 --no-pager
```

Use `grep -E` if `rg` is unavailable. Check deployed `mini-sub2api --version`; redact addresses,
hostnames and credentials before sharing. Do not dump environment variables, vaults or traffic bodies.

## Starting profile for a 1 GiB host

Start with one or two small text requests at a time, then measure combined peak memory.
Images, large tool outputs and long histories may need more RAM or larger budgets.

```bash
export MINI_SUB2API_LIMITS='{"requestBytes":4194304,"outputBytes":4194304,"outputItems":1024,"globalBytes":134217728,"keyBytes":100663296,"sessionBytes":67108864,"sessionRecords":256}'
export GOMEMLIMIT=96MiB
build/bin/mini-sub2api serve
```

This sets 4 MiB request/output limits, 128/96/64 MiB global/Key/session budgets, 1,024 output items
and 256 records/session. The default global budget is 2 GiB; neither profile caps process RSS or
adapts to host RAM. API-key traffic, pre-admission work, JSON trees, identities, TLS and allocator
overhead are not fully covered. Subscription reserves roughly eight times encoded input size plus
settings/output costs. A request below the body limit can still fail admission; eviction can require
the client to resend full history. Oversized traffic fails without truncation.

[`GOMEMLIMIT`](https://go.dev/doc/gc-guide#Memory_limit) is a soft Go-runtime limit; it does not cap
Rust or all Go RSS. The profile is a starting point, not a guarantee that every workload fits.

For systemd, use the managed environment file or this drop-in when no environment file overrides it:

```ini
[Service]
Environment='MINI_SUB2API_LIMITS={"requestBytes":4194304,"outputBytes":4194304,"outputItems":1024,"globalBytes":134217728,"keyBytes":100663296,"sessionBytes":67108864,"sessionRecords":256}'
Environment=GOMEMLIMIT=96MiB
MemoryAccounting=yes
Restart=on-failure
RestartSec=3s
```

Load changes with a planned service restart, which interrupts streams and clears in-memory history.
The coordinator restarts an exited Core; systemd handles coordinator failure. Neither prevents OOM.

After measurement, optional `MemoryHigh`/`MemoryMax` can constrain the whole service group, with
headroom for normal peaks and the host. `MemoryHigh` applies pressure; `MemoryMax` can cause an OOM
kill. Do not copy context budgets directly into those settings. Increase RAM or reduce workload if
needed; swap absorbs transient pressure at a latency cost. Check disk space before adding swap.
Measure representative load and idle recovery after the three-hour history expiry. Allocation-free
JSON size counting reduces temporary buffers; retained history and other request copies still cost RAM.

## Separate boot failures

- Xray: inspect `systemctl status xray.service --no-pager -l` and
  `sudo journalctl -u xray.service -b -n 100 --no-pager`; use its actual paths for
  `xray run -test -config <config-file>` ([reference](https://xtls.github.io/en/document/command)).
  Config validation does not check runtime permissions/port conflicts. An outbound proxy failure
  can independently break connectivity.
- SSM: an unconfigured instance-management role concerns AWS credentials. Follow the
  [AWS permissions guide](https://docs.aws.amazon.com/systems-manager/latest/userguide/setup-instance-permissions.html)
  for an EC2 instance profile or Default Host Management Configuration; it does not repair OOM or Xray.
