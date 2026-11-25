# Device Faker Server — egress proxy diagnostics (English summary)

> **Scope**: this page covers only what operators need when debugging the **egress proxy**.
> The complete server manual (build, install, ports, tokens, troubleshooting) is
> [`docs/SERVER.md`](../SERVER.md) (Chinese); the mechanism itself is described in
> [`CONFIG.md`](CONFIG.md) under "Network Egress".

## 1. Files and permissions

| Path | Purpose | Mode |
| --- | --- | --- |
| `/data/adb/device_faker/config/egress.toml` | proxy definitions + probe policy (**contains passwords**) | `0600` |
| `/data/adb/device_faker/config/backups/egress.toml.<stamp>-<rev8>.bak` | pre-write backup (10 kept) | `0600` |
| `/data/adb/device_faker/config/config.toml` | `[egress].default` + `[[apps]].egress` references only | `0644` |

- A widened mode is healed back to `0600` on read, with the `egress_perm_fixed` warning.
- `egress.toml` is never part of `GET /config?format=toml` and is never exported by the WebUI;
  the API only ever exposes `hasCredentials`.
- `backupId` in responses is the `<stamp>-<rev8>` part of the backup file name
  (e.g. `20261201T030000.987654321Z-5b1f2c3d`).

## 2. Replay diagnostics

Logcat tag `DeviceFaker`; the same text goes to the module log when `debug = true` in `config.toml`.

```
[egress] replay hook armed package=<pkg>
[egress] replay start package=<pkg> trigger=post_zygote_setpriority
[egress] replay done package=<pkg> probes=118 replays=1 divergences=1 state=match elapsed=2001ms
[egress] replay hook unavailable reason=commit_failed error=...
[egress] replay hook not fired within 2000ms (no replay started); module retained (code pages stay mapped for this process) package=<pkg> waited=2000ms
```

| Line | Meaning |
| --- | --- |
| `replay hook armed` | the PLT hook is installed on `libandroid_runtime.so`; the process now keeps the module mapped |
| `replay start` / `replay done … state=match` | the hook fired and the replay ran to the end of the 2000ms window (118 probes) |
| `replay hook unavailable reason=...` | the hook could not be installed → this process only got one-shot injection |
| `replay hook not fired within 2000ms ... module retained ...` | armed, but neither trigger point ever fired (see below) |

`replay hook not fired within …` has **two trigger points** (neither depends on the hook, neither
creates a thread):

1. **timer at the end of the replay window** — `alarm(⌈REPLAY_WINDOW_MS/1000⌉)` (2000ms by default);
   when it fires without a replay, the line is logged once and `SIGALRM` is immediately restored;
2. **process-exit fallback** — an `atexit` callback logs the same line if the process exits inside the
   window (`waited=` is then the real elapsed milliseconds).

- **The normal path logs nothing**: the hook usually fires in ~26ms, cancels the timer with `alarm(0)`
  and suppresses the exit fallback, so a healthy process shows no such line.
- **The module code pages stay mapped** after it is logged: an armed process skips `dlclose`, and
  unloading here would crash (the replay thread and the hook live in that `.so`). The module is still
  invisible by name in `/proc/<pid>/maps` (anonymous mapping) — see [CONFIG.md](CONFIG.md).
- **SIGKILL** (the usual way AMS kills a process) runs neither trigger — the only signal is the absence
  of `replay start` in logcat.

## 3. Uninstall and disable

- `uninstall.sh` stops the supervisor → stops the server → clears the system proxy
  (`settings put global http_proxy :0`) → removes `/data/adb/device_faker`.
- **Disabling the module by hand does not run `uninstall.sh`**: the system proxy stays in Android
  settings and nothing will clear it. Run `DELETE /api/v1/egress` (or the WebUI switch) first;
  as a last resort with root: `su -c 'settings put global http_proxy :0'`.

## 4. Credentials

- The API, logs, audit and WebUI never contain proxy passwords; the only signal is `hasCredentials`.
- `dry_run` / `diff` responses mask `username` / `password` values as `<redacted>` (intentional).
- Authenticated proxies are **not supported** for app traffic in this release (libcore does not send
  `java.net.socks.*` credentials and HTTP Basic is not implemented): use an IP allow-list, or wait for
  the P2 in-device authenticated relay.
