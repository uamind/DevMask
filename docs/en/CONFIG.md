# Configuration Guide

Configure a fake device identity and system properties per app. The config file uses TOML format.

## Config File Path

- `/data/adb/device_faker/config/config.toml`
- `/data/adb/device_faker/config/egress.toml` (**0600**, proxy definitions and probe policy; never exported, never returned by the API — see [Network Egress](#network-egress-egress))

The config is **hot-reloaded**: the module re-reads the file on every app launch. After editing, just restart the target app — **no system reboot required**.

- If the config file is missing or fails to parse, the app skips spoofing and the module unloads; other apps are unaffected
- Apps not present in any config are not spoofed at all

## Global Settings

```toml
debug = false                        # debug logging (off by default)
default_force_denylist_unmount = false
```

- `debug`: when enabled, Info-level logs are output (Error-only when disabled), written to `/data/adb/device_faker/logs/device_faker.log`; keep it disabled during normal use for better stealth
- `default_force_denylist_unmount`: enables Zygisk's `FORCE_DENYLIST_UNMOUNT` for the target apps; can be overridden per template / per app with `force_denylist_unmount`

## Editing the Configuration

> Multi-user note: append `@userId` to a package name to target a specific user only.
> 
> - `userId` is the number in the path `/data/user/<userId>/...` (e.g. `0`, `999`)
> - Match priority: `com.example.app@userId` is tried first, then falls back to `com.example.app`
> - This applies to both `package` in `apps` and the `packages` list of templates
> - **A bare package name (no `@userId`) has two meanings: "the config for user 0" + "the fallback config for every clone / multi-account user (non-zero)"**. So when the main app and its clones need
>   different templates, put the bare name in one template only and write `com.example.app@999` for the clone in another; **never put the same bare name in two templates** (at runtime only the
>   first one in file order wins, and the WebUI warns about duplicate package names). Conversely, editing the bare name affects every clone that relies on the fallback.
>
> Scope cheat sheet (runtime lookup order is `pkg@uid` → bare `pkg`, first hit wins):
>
> | Form | Scope |
> | --- | --- |
> | `com.example.app` | user 0 **+ all clones** (clones fall back here when they have no own config) |
> | `com.example.app@0` | **user 0 only** (the exact key wins over the bare name, so clones never match it) |
> | `com.example.app@999` | **that clone only** (user 999) |
>
> Use `@0` when only the main app should be spoofed and clones must not inherit; use the bare name when the main app and all its clones should be spoofed.

### Method One: Device Templates

Define a `packages` list in a template; it is automatically applied to all listed packages:

```toml
[templates.redmagic_9_pro]
packages = [
    "com.mobilelegends.mi",
  # Only applies to userId=999
  # "com.mobilelegends.mi@999",
    "com.supercell.brawlstars",
]
manufacturer = "ZTE"
brand = "nubia"
model = "NX769J"
device = "REDMAGIC 9 Pro"
fingerprint = "nubia/NX769J/NX769J:14/UKQ1.230917.001/20240813.173312:user/release-keys"
build_id = "UKQ1.230917.001"

# No [[apps]] needed — all listed packages automatically use this template
```

**Advantages**:
- ✅ Centralized management of device identity and package lists
- ✅ No need to repeat [[apps]]
- ✅ See at a glance which apps use which template

### Method Two: Direct Configuration

Use `[[apps]]` to specify device info for a single app:

```toml
[[apps]]
package = "com.omarea.vtools"
manufacturer = "Xiaomi"
brand = "Xiaomi"
model = "2509FPN0BC"
device = "Xiaomi 15 Pro"
product = "popsicle"
name = "popsicle"
```

**Note**: `package` is the only **required** field; if missing, the whole config fails to parse.

### Priority

```
[[apps]] direct configuration > template packages list > global defaults
```

- **`[[apps]]` matches the whole record**: once a package matches an `[[apps]]` entry, that record **entirely replaces** the template — fields not set in the record do **not** fall through to the template, only global defaults apply
- If no `[[apps]]` entry matches, the template `packages` lists are searched
- Finally, global defaults are applied: `force_denylist_unmount` ← `default_force_denylist_unmount`
- Apps matching nothing: no spoofing, module unloads

**Template override example**:

```toml
[templates.redmagic_9_pro]
packages = ["com.mobilelegends.mi"]  # uses this template by default
manufacturer = "ZTE"
brand = "nubia"

[[apps]]
package = "com.mobilelegends.mi"  # on match, the whole record replaces the template
manufacturer = "Samsung"
# Fields set in the template but not here (e.g. brand) do NOT take effect
```

## Field Reference

### Device Information Fields

The following fields can be used in templates or `[[apps]]`. Each field rewrites both the `android.os.Build` field read inside the app and the matching system properties, so the two stay consistent:

| Field | Build field | System properties | Description |
|-------|-------------|-------------------|-------------|
| `manufacturer` | `Build.MANUFACTURER` | `ro.product.manufacturer` + partition variants | Manufacturer (e.g. Xiaomi, Samsung) |
| `brand` | `Build.BRAND` | `ro.product.brand` + partition variants | Brand (e.g. Redmi, nubia) |
| `marketname` | — (properties only) | `ro.product.marketname` + `ro.vendor.oplus.market.name` | Market name (e.g. Nothing Phone (3)); OnePlus/OPPO devices read the latter |
| `model` | `Build.MODEL` | `ro.product.model` + partition variants | Model number (e.g. NX769J, SM-S9280) |
| `name` | — (properties only) | `ro.product.name` + partition variants | Internal product name (e.g. popsicle) |
| `device` | `Build.DEVICE` | `ro.product.device` + partition variants | Codename (e.g. xuanyuan); falls back to `product` / `name` when unset |
| `product` | `Build.PRODUCT` | `ro.product.name` + partition variants, plus `ro.build.product` | Product codename (e.g. xuanyuan); same source as `name` — each fills in the other when unset |
| `hardware` | `Build.HARDWARE` | `ro.hardware` | Hardware name (e.g. qcom); no partition variants |
| `board` | `Build.BOARD` | `ro.product.board` | Board name (e.g. kalama); no partition variants |
| `soc_model` | `Build.SOC_MODEL` | `ro.soc.model` | SoC model (e.g. SM8550); no partition variants |
| `fingerprint` | `Build.FINGERPRINT` | `ro.build.fingerprint` | Fingerprint |
| `build_id` | `Build.ID` | `ro.build.id`, `ro.system.build.id`, `ro.vendor.build.id`, `ro.product.build.id` | Build ID (e.g. UKQ1.230917.001) |
| `characteristics` | — (properties only) | `ro.build.characteristics` | Characteristics (e.g. tablet) |

> **"Partition variants"** means `ro.product.{odm,vendor,system,system_ext,product,bootimage,system_dlkm,vendor_dlkm,odm_dlkm}.<field>`: different vendor ROMs read device info from different partitions, so the whole family has to be written for every read path to see the same value.
>
> Keys that **do not already exist** on the device are never written — e.g. some ROMs have no `ro.product.system_dlkm.model`, and non-OPPO devices have no `ro.vendor.oplus.market.name`. This keeps properties a real device lacks from appearing, without hurting the spoof. **Exception**: keys written explicitly in `custom_props` are always written.

> **A fingerprint fills in the other fields**: writing only `fingerprint` is enough for a consistent spoof — the module parses it and fills in any of `brand`, `product`, `device`, `android_version`, `build_id` and `incremental` you did **not** set explicitly (values you did write are never overwritten). The derived `incremental` drives both `Build.VERSION.INCREMENTAL` and `ro.build.version.incremental`.

### Android Version and Build Info Spoofing

| Field | Build field | System properties | Examples |
|-------|-------------|-------------------|----------|
| `android_version` | `Build.VERSION.RELEASE` | `ro.build.version.release`, `ro.system.build.version.release`, `ro.vendor.build.version.release`, `ro.product.build.version.release` | `"15"`, `"14"`, `"13"` |
| `sdk_int` | `Build.VERSION.SDK_INT` (integer) | `ro.build.version.sdk`, `ro.system.build.version.sdk`, `ro.vendor.build.version.sdk`, `ro.product.build.version.sdk` | `35`, `34`, `33` |
| `display_id` | `Build.DISPLAY` | `ro.build.display.id` | `PD2546_16.0.9.400(CN01)`; no partition variants |
| `incremental` | `Build.VERSION.INCREMENTAL` | `ro.build.version.incremental` + partition variants | `compiler260228234011` |
| `security_patch` | `Build.VERSION.SECURITY_PATCH` | `ro.build.version.security_patch` + partition variants | `2025-06-05` |

> `security_patch` is written together with all of its partition copies.

### Per-app DPI

Set `dpi` (range `120`–`640`) in a template or `[[apps]]`, for example `dpi = 420`. The current density override is saved first; it is restored right after the target app goes to the background or exits, and reapplied when the app returns to the foreground. If there was no override, it is reset with `wm density reset`.

### Custom Properties

`custom_props` sets arbitrary system properties (written verbatim, no partition-variant expansion), usable in both templates and `[[apps]]`, and supports [special marker values](#special-marker-values):

```toml
[[apps]]
package = "com.custom.app"
manufacturer = "Custom"

[apps.custom_props]
"ro.custom.property" = "custom_value"
"ro.debug.mode" = "__DELETE__"    # delete this property
"ro.empty.value" = "__EMPTY__"    # set to empty string
```

### Special Marker Values

| Marker value | Meaning |
|--------------|---------|
| Normal string | Set to that value |
| `""` | Set to an empty string (equivalent to `__EMPTY__`) |
| `"__EMPTY__"` | Set to an empty string |
| `"__DELETE__"` | Delete the property (for structured fields: the whole property family; **only actually applied when `companion_resetprop = true`**) |

> **Structured fields** (`brand`, `model`, `fingerprint`, `android_version`, …) with `__DELETE__`: the field is no longer written and the whole group of properties behind it is deleted; `__EMPTY__` writes an empty string (the Build field becomes an empty string too). A few fields also handle vendor aliases (e.g. `marketname`, `security_patch`).
>
> `sdk_int` is written as `sdk_int = "__DELETE__"` (a string marker on a numeric field; the WebUI form only accepts integers, so edit the TOML by hand) — **reads depending on it will fail afterwards, so treat it as high risk**.
>
> ⚠️ **Deletion only runs with `companion_resetprop = true`**: under the default mode the delete list is skipped (`__DELETE__` conflicts with "only this app is affected"). `__EMPTY__` still works in the default mode.

### Feature Switches

| Field | Default | Description |
|-------|---------|-------------|
| `companion_resetprop` | `false` | When `true`, the spoofed values apply **system-wide** (`getprop` and other apps see them) and are restored once the app goes to the background or exits; when `false` (default) spoofing stays inside the app's own process. Enable it only when you need the system-wide effect; see [How It Takes Effect](#how-it-takes-effect) |
| `dpi` | — | Temporarily changes the system display density (range `120`–`640`); restored automatically once the app goes to the background or exits |
| `force_denylist_unmount` | inherits `default_force_denylist_unmount` | Force-enable Zygisk `FORCE_DENYLIST_UNMOUNT` for this app; priority: app > template > global default |

**Notes**:
- All fields except `package` are optional
- When using a template's `packages`, no `[[apps]]` entry is needed (auto-applied)
- An `[[apps]]` record replaces the template entirely; unset fields do not fall through (see [Priority](#priority))
- Fields not listed in the tables above are silently ignored (they do not affect parsing)

## Network Egress (egress)

> Requires `device_faker_server` v1.10.0+ (used by the WebUI "Network egress" card and probing). With only the module installed the config still applies — you just lose probing and read-back verification.

Egress decides **which IP app traffic leaves from**; it is independent of device-identity spoofing. Two granularities:

| Level | Mechanism | Scope | Protocol |
| --- | --- | --- | --- |
| Device-wide (plan A) | writes the Android global proxy (`settings global http_proxy`) | apps that honour the system proxy (OkHttp / HttpURLConnection / WebView / most SDKs) | HTTP/HTTPS CONNECT only, **no proxy auth** |
| Per-app (plan B) | Zygisk injects `http/https/socksProxy` and `ProxySelector` inside the target process | that one app only, system settings untouched | HTTP + SOCKS5; **authenticated proxies do not work in this release** (see known limitations) |

### config.toml: references only

```toml
[egress]
default = "office_http"        # device-wide default; omit or leave empty for direct

[[apps]]
package = "com.example.a"
egress  = "lab_socks"          # per-app override (SOCKS5 here)

[[apps]]
package = "com.example.b"
egress  = "__DIRECT__"         # force direct, overriding the device-wide proxy
```

| Location | Key | Value | Notes |
| --- | --- | --- | --- |
| `[egress]` | `default` | proxy name | Device-wide exit. **Only `default` is accepted**; `host` / `port` / `username` / `password` → `422 unknown_field` (credentials live only in `egress.toml`) |
| `[[apps]]` | `egress` | proxy name or `"__DIRECT__"` | Per-app override. Omitted = inherit `[egress].default`; `__DIRECT__` = force direct |

Inheritance chain (only two levels): `[[apps]].egress` → `[egress].default` → direct. Templates do **not** take part.

Proxy names: `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$` (1–64 chars, case-sensitive); `__DIRECT__` is reserved. Referring to a proxy that is not defined yet is **allowed** (the two files are decoupled); at runtime you get an `egress_proxy_not_found` warning and the app goes direct.

> **Templates do not support `egress`**: a template is shared by several packages, so it is not a per-app exit. Giving a template-bound app a per-app egress creates an inline record for it, and an inline record **replaces the template wholesale** (identity fields are lost) — the write returns an `egress_inline_shadows_template` warning. To keep both identity and egress, write the identity fields into that `[[apps]]` record explicitly.

### egress.toml: proxy definitions and probe policy

Path `/data/adb/device_faker/config/egress.toml`, **mode 0600** (root only). `config.toml` stays 0644 (shareable / exportable), so **passwords live only here**. The service maintains this file; after editing by hand, restart the service.

```toml
version = 1

[proxies.office_http]
type     = "http"              # http | socks5
host     = "198.51.100.10"     # hostname or IPv4; no scheme / port / path
port     = 8080
# username = "<USERNAME>"      # optional
# password = "<PASSWORD>"      # optional; never appears in any API / log / audit / WebUI

[proxies.lab_socks]
type     = "socks5"
host     = "198.51.100.20"
port     = 1080
username = "<USERNAME>"
password = "<PASSWORD>"

[probe]
url             = "http://checkip.example.com/"   # http:// only (no TLS stack)
timeout_seconds = 5                                # 1–30, default 5
fail_open       = true                             # allow direct when the proxy is down (default)
```

- At most 64 definitions; `host` ≤253 chars, `username` ≤128, `password` ≤256.
- If permissions get widened, the service heals them back to 0600 and reports `egress_perm_fixed`; backups (`backups/egress-*.toml`) are 0600 as well.
- `egress.toml` is **not** part of `GET /config?format=toml` and is not exported by the WebUI; `GET /api/v1/egress` only returns redacted summaries (`hasCredentials` is the only credential signal).
- `username` / `password` are **stored securely** (0600; the API only reports `hasCredentials`), but **app traffic never sends them**: the SOCKS5 properties are readable while libcore does not send credentials, and HTTP Basic is not implemented. Use an **IP allow-list** on the proxy, or wait for the P2 "in-device authenticated relay". See [known limitations](#known-limitations).

### fail-open and degradation

With `fail_open = true` (default), the service switches the system proxy to direct (`settings put global http_proxy :0`) and flips `health.degraded` once a proxy is considered unusable — the config itself is untouched, and one successful probe restores it:

1. an explicit probe fails with `connect` / `dns` / `timeout` / `tls` (`http_status` / `invalid_response` do not trigger it — the path may be fine);
2. the read-back after a write does not match the expectation;
3. the read-back during the startup self-heal does not match.

The state is visible in `health` (`degraded` / `degradedSince` / `degradedReason`) of `GET /api/v1/egress` and in the log. With `fail_open = false` (fail-closed) the checks still run but the system proxy is left alone — no network is better than a changed exit IP.

### When it takes effect

| Level | Timing |
| --- | --- |
| Device-wide | **immediately** — new connections go through the proxy (established connections must reconnect) |
| Per-app | **after the target app cold-starts** (same as identity spoofing). Injection happens early in process start and is then corrected by the post-start replay (next section); use `POST /api/v1/apps/{pkg}/restart` if needed |

Clearing the system proxy **must** use `settings put global http_proxy :0` — `settings delete global http_proxy` does not clear it on Android 17 (measured; see [SERVER.md](../SERVER.md)). The service encapsulates this rule; follow it when doing it by hand.

### How per-app injection takes effect (post-start replay)

Per-app egress (`[[apps]].egress`) is **in-process injection plus post-start replay**, not a write-once operation. This order explains most "configured but not effective" reports.

1. **Injection** happens in Zygisk's `pre_app_specialize` (early in process start, before `Application` is constructed): it writes `http.proxyHost/Port`, `https.proxyHost/Port`, `socksProxyHost/Port` and the credential carriers, and tries `ProxySelector.setDefault(...)`.
2. **The framework then overwrites them**: while binding the app it runs `ActivityThread.handleBindApplication` → `ActivityThread.updateHttpProxy(app)` → `Proxy.setHttpProxyConfiguration(cm.getDefaultProxy())`, rewriting the http/https properties (including nonProxyHosts and ProxySelector) from the **device-wide** `ProxyInfo` — and clearing them when the device-wide setting is direct. Measured on a Pixel 9, that overwrite lands **~28–95ms** after injection (95ms in t9, 28ms in this round).
3. **Hence the replay**: a short-lived thread started after injection reads the properties back on a fixed schedule and rewrites the **whole** expected set whenever it sees a mismatch.

The replay window is **0–2000ms with no early exit** — we cannot know when the framework writes, and stopping early would miss a later overwrite:

| Range (relative to injection) | Cadence | Purpose |
| --- | --- | --- |
| `0..=160ms` | every 2ms | hot zone: restore right around the framework overwrite |
| `165..=300ms` | every 5ms | slower devices / slower cold starts |
| `300..=2000ms` | geometric back-off (from 340, ×5/4+10, last point 2000) | backstop for very slow cold starts |

That is **118 probes** in total; the thread sleeps most of the time, so the CPU cost is a few dozen property reads.

**Why the trigger is not "spawn a thread in specialize"**: creating a thread during `pre_app_specialize` makes zygote's `selinux_android_setcontext` (which requires a single-threaded process) fail and the process aborts with SIGABRT (Pixel 9 / Android 17 tombstone: `JNI FatalError called: ... selinux_android_setcontext(...) failed`). So the thread is started from a PLT hook once the process transition is done: `setpriority` on `libandroid_runtime.so` (AOSP always calls `setpriority(PRIO_PROCESS, 0, PROCESS_PRIORITY_DEFAULT)` after `ZygoteHooks.postForkChild`/`SetProcessState`), with `__system_property_get` as the standby trigger.

**Explicit-only injection**: only processes whose `[[apps]].egress` names a proxy or `__DIRECT__` get the hook and the replay thread. Omitted / empty → **no injection** (the device-wide behaviour is already written by the framework into every process; injecting again would only add a hook, a thread and the need to keep the module mapped per process). If the named proxy does not exist in `egress.toml`, nothing is injected and there is **no fallback to the device-wide default** — direct is safer than silently switching the exit IP.

Observability (requires `debug = true`; goes to both logcat and the module log):

```
[egress] replay hook armed image=libandroid_runtime device=... inode=... symbols=setpriority,__system_property_get   # module log (debug = true)
[egress] replay hook armed package=com.example.a                                                                  # logcat (DeviceFaker tag)
[egress] replay start package=com.example.a trigger=post_zygote_setpriority
[egress] replay package=com.example.a t=28ms reason=framework_overwrite applied=6/6 changed=[http.proxyHost=-->127.0.0.1, http.proxyPort=-->18082]
[egress] replay done package=com.example.a probes=118 replays=1 divergences=1 state=match elapsed=2003ms
[egress] replay hook unavailable reason=commit_failed error=...
[egress] replay hook not fired within 2000ms (no replay started); module retained (code pages stay mapped for this process) package=com.example.a waited=2000ms
[egress] proxy_selector degraded reason=of_method_missing sdk=Some(37)
```

`state=match` means every property was in the expected state when the window closed; `state=mismatch` means it was not (e.g. the thread was suspended for too long); `replay hook unavailable` means no hook was installed and this process fell back to one-shot injection.

`replay hook not fired within …` is the diagnostic for "the hook was armed but neither trigger ever fired". It has **two trigger points**, neither of which depends on the hook and neither of which creates a thread:

1. **Timer at the end of the replay window**: after arming, an `alarm(⌈REPLAY_WINDOW_MS/1000⌉)` is installed (window = 2000ms by default). If no replay has been spawned by the time it fires, the line is logged once and `SIGALRM` is **immediately** restored to its previous disposition.
2. **Process-exit fallback**: if the process exits inside the window, an `atexit` callback logs the same line (`waited=` is then the real elapsed milliseconds; the timer path reports the window value).

**The normal path logs nothing**: the hook usually fires in ~26ms and starts the replay thread; on a hit the code calls `alarm(0)` to cancel the timer, restores the signal disposition and marks the probe as reported — so this line **never appears** in a healthy process (the log stays identical to a build without the diagnostic). The diagnostic is logged at most once; both trigger points share the same flag.

**The module code pages stay mapped after the line is logged**: a process that armed the hook already skips `dlclose`, and it **must not** be unloaded here — the replay thread and the hook live in that `.so`, so unloading would crash. That is exactly what the `module retained (code pages stay mapped for this process)` wording in the line means. The module is still **invisible by name** in `/proc/<pid>/maps` (anonymous mapping, see Known limitations).

Seeing this line means the process only got the pre-`specialize` one-shot injection, so per-app HTTP egress is likely overwritten by the framework afterwards: check whether the ROM changed the call order in `SpecializeCommon`, or whether the hook is being blocked. Note that **SIGKILL** (the usual way AMS kills a process) runs neither trigger point — in that case the only signal is the absence of `replay start` in logcat.

### Known limitations

- **Authenticated proxies do not work in this release**: `egress.toml` credentials are stored safely, but **app traffic never sends authentication** — the injected `java.net.socks.username/password` properties are readable inside the process while libcore's SOCKS client never sends them (measured on Android 17: proxy side `methods=0002 user_ok=0 pass_ok=0` → `auth-failed`), and HTTP Basic is not implemented (only host/port are injected, with an `[egress] unsupported-auth` line). Use a proxy authorised by **IP allow-list**, or wait for the P2 "in-device authenticated relay" (not implemented). Note that **probing** uses the service's own client (SOCKS5 auth works for the device-side probe; the Manager probe only supports HTTP proxies), so "probe succeeded" does not mean "apps can reach the network".
- **A process with explicit `egress` keeps module code pages resident**: the replay thread and the PLT hook live in the module `.so`, so zygisk cannot `dlclose` it while they run. They are **not visible by name in `/proc/<pid>/maps`**: Zygisk maps the module anonymously (an armed process showed 0 hits for `device_faker`/`arm64-v8a`/`libzygisk` across 3170 lines; only the **companion** process shows `/data/adb/modules/device_faker/zygisk/arm64-v8a.so`). Real detection must compare **anonymous executable segments** (`r-xp`) against a process without egress. Processes **without** per-app egress still unload the module immediately.
- **Changing the device-wide proxy while an app is running is not replayed**: the window only covers 0–2000ms after a cold start. Changing `[egress] default` later (the AMS `updateHttpProxy` broadcast) lands outside it — restart the app to pick it up.
- **When the `setpriority` hook cannot be installed the injection degrades to one-shot**: if `libandroid_runtime.so` is missing or the PLT hook commit fails, a single `[egress] replay hook unavailable reason=...` line is logged and no replay happens — the values are still written but may be overwritten by the framework's `updateHttpProxy` (symptom: per-app HTTP egress ineffective, `__DIRECT__` unable to restore direct).
- `ProxySelector.of(...)` does not exist on Android 17 (SDK 37): the property path is authoritative and the selector is a bonus; a single `[egress] proxy_selector degraded reason=of_method_missing` line is logged per process and injection/replay are unaffected.
- Device-wide covers HTTP/HTTPS CONNECT only: **SOCKS5 and proxy auth work only via plan B**. Pointing the device-wide default at a proxy with credentials returns `egress_credentials_ignored` (it still applies as `host:port`).
- UDP / QUIC are not constrained by an HTTP proxy; established connections do not switch paths; VPN apps override the system proxy.
- WebView follows the system proxy (plan A works, plan B does not); native stacks (cronet, custom C++, QUIC) are the opposite.
- **Run `DELETE /api/v1/egress` before uninstalling or disabling the module**: the system proxy lives in Android settings and nothing can clear it once the module is disabled. `uninstall.sh` clears it after stopping the service, but toggling the module off by hand does not run it.
- Probing defaults to `http://checkip.amazonaws.com/`, only accepts `http://`, and runs inside the **service process** (`scope:"app"` results are approximate).

## How It Takes Effect

All apps go through the same unified flow (no mode selection needed):

```
① Overwrite the Build fields → ② Rewrite the system properties → ③ Spoof DPI → ④ Unload the module
```

- **Default (recommended)**: spoofing stays **inside the app's own process** — the app reads the spoofed values while other apps, system services and `getprop` still see the real ones; the module unloads right after patching, leaving nothing resident.
- **`companion_resetprop = true`**: the spoofed values become **system-wide** (`getprop` and other apps see them too). They are restored as soon as the app goes to the background or exits, and reapplied when it comes back.

Enable `companion_resetprop` only when detection tooling (usually reading through a `getprop` subprocess) must also see the spoofed values — the trade-off is that the spoofed properties are globally visible while the app runs.

## Complete Configuration Example

```toml
# ── Global settings ───────────────────────────────────────
debug = false                        # debug logging (off by default)
default_force_denylist_unmount = false

# ── Device templates ──────────────────────────────────────
[templates.redmagic_9_pro]
packages = [
    "com.mobilelegends.mi",
  # Only applies to userId=999
  # "com.mobilelegends.mi@999",
    "com.supercell.brawlstars",
]
manufacturer = "Nubia"
brand = "REDMAGIC"
model = "NX809J"
device = "REDMAGIC 11 PRO"
product = "NX809J"
fingerprint = "REDMAGIC/NX809J-UN/NX809J:16/BP2A.250605.031.A3/20251017.000000:user/release-keys"
build_id = "BP2A.250605.031.A3"

# ── Direct configuration ──────────────────────────────────
[[apps]]
package = "com.omarea.vtools"
manufacturer = "Xiaomi"
brand = "Xiaomi"
model = "2509FPN0BC"
device = "Xiaomi 15 Pro"
product = "popsicle"
name = "popsicle"
android_version = "15"
sdk_int = 35
dpi = 420                       # temporary display density (120–640)
force_denylist_unmount = true  # overrides the global default, this app only

[[apps]]
package = "com.example.detected.app"
companion_resetprop = true     # system-wide consistent properties (restored automatically on app exit)
manufacturer = "Custom"

[apps.custom_props]
"ro.custom.property" = "custom_value"
"ro.debug.mode" = "__DELETE__"
```
