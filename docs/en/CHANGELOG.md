# 📝 Changelog

## ⬆️[v1.10.0 → v1.11.0] - 2026-12-02

### 🆕 New Features
- 🔐 **In-device authenticated relay**: proxies with a username/password are forwarded by a relay that only listens on `127.0.0.1` and adds HTTP Basic / SOCKS5 RFC1929 upstream; covers device-wide and per-app egress with zero app changes
- 🔌 Relay port table: a `[relay]` section plus read-only `local_port` in `egress.toml`; range 17800–17899, device-wide port = `base_port`, per-app ports assigned by a stable hash
- 🛡️ uid firewall: three iptables chains (`DF_EGRESS_RLY` / `_ALLOW` / `_DENY`) that only let authorized uids connect; rebuilds are **fail-closed** (DENY catch-all, no naked window)
- 🔁 `POST /api/v1/egress/relay/restart` (`ops`, forced apply); `GET /egress` gains a `relay` object; `GET /egress/proxies` gains `localPort` / `relayActive`
- 🔍 Probe gains `via=relay` (default stays `upstream`), covering "local port listening + relay forwarding + upstream auth + target round-trip"; new `relay_not_listening` / `relay_connect` reasons

### ⚡ Improvements
- 🚫 **Fail-closed injection**: an invalid relay view (non-loopback host / out-of-pool port / bad type) now injects `127.0.0.1:9` and arms the replay instead of behaving as "not configured" — the app goes offline but never silently changes exit
- 🧯 Narrowed fallback: only "relay explicitly disabled" (`[relay].enabled=false` or an unmigrated file) falls back upstream; enabled but missing `local_port` is fail-closed
- 🧹 Credential boundary: from v1.2 credentials are **never written into app processes** (≤v1.1 wrote `java.net.socks.username/password`, which never worked in practice)
- 📊 Manager: `DeviceSummary.egress.relay = {enabled, active, listeners, firewallApplied}`; `/devices/{id}/egress` passes `relay` through; `socks5` probing now really connects (reqwest `socks` feature)
- 📚 Docs: API.md / openapi.yaml (30 operations) / CONFIG.md / SERVER.md / Manager contract and architecture; five new `egress_relay_*` warning codes (including `egress_relay_device_socks5_unsupported`)

### ⚠️ Known Limitations
- Relay ports (`127.0.0.1:178xx`) and the `DF_EGRESS_RLY` chain names are **detectable**; relay traffic leaves as **root**, so per-uid firewalls such as AFWall+ / NetGuard no longer block that app
- **Device-wide egress depends on the relay being alive**: while a listener is down the whole-device proxy path is unavailable; with `fail_open=true` the next failed upstream probe silently falls back to direct (set `[probe].fail_open=false` if you must never leak)
- Only **credentialed** proxies use the relay; there is no API to disable it (edit `[relay].enabled` in the 0600 `egress.toml` and restart)
- The relay only covers the **Java layer**: native stacks and WebView are not protected by fail-closed
- 🪜 **Layered uid policy (availability vs. security)**: per-app listeners decide by uid after accept; an **unknown uid is allowed only when `relay.firewall.applied=true`** (a fresh `iptables -S` sample) and logs a rate-limited `uid_unresolved_firewall_enforced` line, otherwise it is closed (fail-closed). The allow path revalidates `-S` at most once per second, so a manual `iptables -F` flips it back within ~1s. Trade-off: root-trust (the check constrains who may connect, not which uid leaves)
- ⚠️ **The device-wide default must be http**: a merged non-http default is rejected with `422 invalid_egress_default` (regardless of `[relay].enabled` or credentials); an existing one is blackholed to `127.0.0.1:9` only while the relay is enabled (`enabled=false` is treated as unconfigured, direct) — **no** SOCKS5 listener is created on `base_port` in either case, and `egress_relay_device_socks5_unsupported` is reported. Use socks5 definitions through `[[apps]].egress` only
- ⚠️ **Local port hijack window**: between releasing a listener and rebinding, another local process may grab the port. Detection is the next bind failing with `addr_in_use`: per-app reallocates and reports `relay.portChanges` / `restartRequiredApps`, the device-wide port then does not listen (`egress_relay_bind_failed`)

## ⬆️[v1.9.0 → v1.10.0] - 2026-12-01

### 🆕 New Features
- 🌐 **Egress proxy**: device-wide HTTP/HTTPS proxy written to Android global settings (`settings global http_proxy`), so app traffic leaves through a chosen exit
- 🧩 Per-app egress: Zygisk injects `http/https/socksProxy` plus `ProxySelector` inside the target process — **SOCKS5** supported with one exit per app; `__DIRECT__` forces direct (actively overrides the system proxy)
- 📡 Exit-IP probe: `POST /api/v1/egress/probe` requests a configurable probe URL over the effective path and returns exit IP / latency / timestamp, with structured error codes on failure
- 🗄️ Manager proxy pool: `/relay/api/v1/egress-proxies` CRUD, bindings (device-wide / per-app), batch apply (job `kind=egress`) and manager-side probing
- 🖥️ Both WebUIs: a "Network egress" card on the device settings page (configure + probe + roll back to direct), plus a "Proxy pool" page in the Manager with exit IP / proxy name on device cards
- 🔐 Credential hygiene: proxy definitions live in a separate `egress.toml` (`0600`) and every API / log / audit / UI surface only exposes `hasCredentials`; the Manager encrypts credentials with `manager.key` (AES-256-GCM)

### ⚡ Improvements
- 🧯 Clearing the system proxy uses `settings put global http_proxy :0` only (`settings delete` does not clear it on Android 17 — measured and documented)
- 🛟 fail-open is on by default: a failed probe or read-back mismatch falls back to direct, so one bad config cannot brick connectivity
- 🔁 Idempotent writes: when the desired state equals the current one nothing is written or backed up and the revision stays put (`changed:false`); replaying the same apply on the Manager reports `egress_unchanged` instead of turning the job partial
- 🧹 Cleanup on uninstall / disable: `uninstall.sh` clears the system proxy after stopping the service, then removes the runtime directory
- 📚 Docs: five egress endpoints in [API.md](../API.md), a "Network egress" chapter in [CONFIG.md](CONFIG.md), five operations in [openapi.yaml](../openapi.yaml), plus Manager contract/architecture updates
- ⚖️ Per-app injection is now **explicit-only**: only processes whose `[[apps]].egress` names a proxy (or `__DIRECT__`) get the PLT hook and the replay thread (the device-wide default no longer adds cost to every app)
- ⚠️ The three costs of the replay mechanism are documented: (1) a process with explicit egress keeps the **module code pages resident** (the replay thread and the PLT hook live in the `.so`) — but they are **not visible by name in `/proc/<pid>/maps`** (Zygisk uses unnamed/anonymous mappings; an armed process showed 0 hits across 3170 lines), so detection must compare anonymous executable segments; (2) changing the device-wide proxy while an app is running falls outside the 0–2000ms replay window, so the app must restart; (3) when the `setpriority` hook cannot be installed the injection degrades to one-shot (`replay hook unavailable`) and may be overwritten by the framework
- ⚠️ **Proxies with a username/password did not work in v1.10.0**: the injected `java.net.socks.username/password` properties are readable inside the process, but libcore never sends them (measured on Android 17: `methods=0002 user_ok=0 pass_ok=0` → `auth-failed`); HTTP Basic is not implemented (only host/port are injected, logged as `unsupported-auth`). Use an IP allow-list on the proxy (since v1.11.0 these proxies work through the in-device relay — see the v1.11.0 section below)

### 🐛 Bug Fixes
- 🔁 Fixed per-app HTTP egress not taking effect inside the target process: the framework (`ActivityThread.handleBindApplication` → `Proxy.setHttpProxyConfiguration`) rewrites the http/https properties from the **device-wide** proxy roughly 28–95ms after injection; a replay thread is now started from a PLT hook (`setpriority`) once the process transition is done, reading back and rewriting on a 0–2000ms schedule (118 probes, no early exit)
- 🚫 Fixed `__DIRECT__` being ineffective while a device-wide proxy is set (same root cause: the clear was overwritten by the framework; the replay restores the direct state)
- 🧩 Fixed the per-process warning caused by `ProxySelector.of(...)` not existing on Android 17 (SDK 37): the capability is probed first and a single `proxy_selector degraded` line is logged when missing (the selector is a bonus; the property path is authoritative)
- 🔒 Fixed a self-deadlock when reading the config snapshot inside a cross-file transaction (shared lock requested while the exclusive flock was already held)
- 🔐 Fixed `dry_run` / `diff` responses leaking proxy passwords: `username` / `password` values in the diff are now redacted per line to `<redacted>` (key names kept; neither the old value nor the new password appears)
- 🎯 Fixed a failed **per-app** probe wiping the device-wide proxy: `scope="app"` failures now only update `health.lastProbe`, the `502` carries `scope` plus an `egress_probe_scope_app` warning, and **no** `settings put global http_proxy :0` is executed — only device-scope failures trigger fail-open
- 📦 Fixed `PUT /egress`'s `apps` semantics: both `apps: []` and `apps: null` clear every `[[apps]].egress` (whole-array replacement); apps not listed lose the `egress` key

## ⬆️[v1.6.0 → v1.7.0] - 2026-11-09

### 🆕 New Features
- 🩺 Property areas unreadable from the app domain can now be spoofed
- 🚀 WebUI now uses KernelSU pm/io interfaces — package enumeration and file I/O without shell forks
- 🔄 Template conversion runs fully in the frontend: import legacy module ZIP / system.prop directly, no CLI or temp files
- 🎯 Online template library now searches by description and fetches one aggregated file instead of per-file requests
- 🧩 Completed custom_props explicit directives and __DELETE__ (soc_model / android_version families, sdk_int deletable)
- 🪟 Foreground detection now uses the TOP set, so split-screen / floating windows are no longer misjudged
- 🏷️ Log now carries a version banner (version + git hash + build time)

### 🐛 Bug Fixes
- 🛡️ Fixed config being unreadable after Zygisk Next 1.5.3's extensive sepolicy.rule changes, which silently disabled all spoofing (config is now read by companion)
- 🎲 Fixed random device model when several templates declare the same package (deterministic by config order, with a WebUI warning)
- 🇨🇳 Fixed installer text falling back to English on Chinese devices
- 🔍 Fixed WebUI clone config resolution; editing a clone no longer wipes the main app entry
- 🧹 Fixed duplicate rows and false "configured" badges for one app instance, with explicit config scope labels
- 🕵️ Property area copies now use anonymous-mapping camouflage
- 🧪 Fixed properties not applying during the cold-start window in hybrid mode (in-process COW now also runs with companion_resetprop)

### ⚡ Improvements
- 🏎️ Zero-fork property reads in companion — existence checks and backups query the property areas directly
- 👻 Never write properties absent on this ROM — no more enumerable ghost properties
- 📦 Module no longer bundles device_faker_cli, shrinking the package
- 🧱 cow_props split into 11 modules for clearer responsibilities
- ⬆️ Dependency upgrades: rsbinder 0.11, 20+ WebUI dependencies, CI now on NDK r30

## ⬆️[v1.5.0 → v1.6.0] - 2026-09-26

### 🆕 New Features
- 🏷️ Added Build.BOARD / Build.SOC_MODEL camouflage at Build layer
- 📅 Added security_patch security patch date spoofing field
- 🆔 Added dedicated display_id / incremental spoofing fields
- 📐 Added per-app DPI spoofing support
- ✏️ WebUI custom property spoofing is now editable
- 📦 Installer script Zygisk detection compatible with unique Zygisk implementations
- 🔗 Follow dialog added official website entry

### 🐛 Bug Fixes
- 🔄 Fixed two foreground gating failures — cold-start race and stale packages.list snapshot
- 🌐 Fixed massive online template library load failures
- 🧹 Fixed residual key values when switches are set back to default
- 🎯 Fixed multi-open package "configured" status misdetection
- 🔒 Property areas now use process-private copies, eliminating rw-p fingerprint in maps
- ⏱️ Background restore sped up to 0.5s

### ⚡ Improvements
- 🏎️ Long property values support in-place update without delete+recreate
- 🤖 rsbinder upgraded to 0.10, compatible with Android 10~17
- 🗑️ Removed CPU spoofing feature
- 🔇 Module and companion fully silent when debug is off
- 🚀 WebUI critical path slimmed and app list performance optimized
- 💾 Installer script keeps existing config by default

## ⬆️[v1.4.0 → v1.5.0] - 2026-08-01

### 🆕 New Features
- 🌍 Added Turkish language translations
- 🧬 Added CPU camouflage
- 👥 WebUI apps page supports multi-user apps display
- 📦 Installer script added Zygisk detection and automatic Chinese/English language switching
- 🔒 New COW property spoofing engine — replaces full mode, per-process isolation with zero residency
- 🔄 companion_resetprop switch replaces resetprop mode
- 🏷️ Added Build.HARDWARE camouflage at Build layer

### ⚡ Improvements
- ⚡ Optimized module underlying performance (by @Tools-cx-app)
- 👆 WebUI supports step-by-step swipe-back navigation
- 🎬 Optimized WebUI page transition animations
- 🏗️ Refactored logging system
- 🎨 Optimized WebUI styles
- 📦 Updated WebUI build dependencies
- 🏗️ Removed WebUI-X API, adopted pure KernelSU WebUI API
- 🏎️ resetprop enabled skip_svc direct write + property area compaction
- 🏗️ Removed mode distinction — unified into single COW + companion execution flow
- 🔄 Refactored and optimized companion resetprop restore mechanism

## ⬆️[v1.3.5 → v1.4.0] - 2026-05-02

### 🆕 New Features
- 🖥️ WebUI apps page supports system app display toggle
- 🌐 WebUI online template library added GitHub source
- 🏷️ Added build_id and Build.ID camouflage support for all three modes
- 📥 WebUI template page added import/export tools and local template export
- 📤 WebUI template card added export button
- 👆 WebUI supports swipe gesture to switch between pages
- ✏️ WebUI template page supports editing existing template names

### 🐛 Bug Fixes
- 🧹 Fixed residual old values when clearing partial fields in WebUI template editor
- 🔄 Fixed state sync issue of custom property camouflage session when switching between foreground and background in resetprop mode

### ⚡ Improvements
- 🔗 Updated to adapt jni 0.22 API
- ⚡ Significantly optimized WebUI page switching performance
- 🏎️ Optimized WebUI app list loading performance
- 🏗️ Refactored the entire WebUI online template library pipeline
- 📏 Used cutting-edge Web technology pretext to optimize text height measurement in online template virtual list
- 🚀 Optimized WebUI first screen page switch responsiveness
- 🎬 Added WebUI page switching animation
- 🦀 Refactored external resetprop binary call into built-in Rust library (by @Tools-cx-app)
- 💾 Improved configuration file backup format

## ⬆️[v1.3.0 → v1.3.5] - 2026-03-20

### 🆕 New Features
- 👁️ WebUI home page added follow module

### 🐛 Bug Fixes
- 🔧 Fixed WebUI scrolling issue caused by changes in Android System WebView rendering logic
- 🔧 Fixed configuration parsing failure due to old Android System WebView not supporting ES2022 standard

### ⚡ Improvements
- 🏗️ Replaced python scripts with xtask for core compilation (by @Tools-cx-app)
- 🌐 Optimized WebUI online template library loading, added wget loading method and retry mechanism
- 🎨 Optimized WebUI app configuration editing interface interaction
- ⚡ Optimized WebUI app list loading display
- 📋 Improved WebUI configuration reading, parsing, editing and saving

## ⬆️[v1.2.0 → v1.3.0] - 2026-01-22

### 🆕 New Features
- 📦 WebUI supports displaying configured but not installed package names
- 🏷️ WebUI online template library added brand classification
- 🤖 Added Android version camouflage and custom attribute features
- 🔍 Added template search function
- 👥 WebUI supports displaying and distinguishing multi-user package names

### ⚡ Improvements
- 🎨 Optimized WebUI styles and layout
- ⚡ Optimized WebUI application list loading
- 🔧 characteristics field supports full mode
- 🔒 Optimized full mode __system_property_get function interception
- 🔄 Refactored WebUI configuration conversion, supports direct selection

## ⬆️[v1.1.0 → v1.2.0] - 2025-12-17

### 🆕 New Features
- 🌐 WebUI added online template library feature
- 🛠️ Added CLI tool for configuration conversion and online template loading
- 🔧 Added resetprop mode support and characteristics property
- 🌍 WebUI added multi-language support
- 📋 Added FORCE_DENYLIST_UNMOUNT support
- 👥 Added support for multi-user environment configuration

### ⚡ Improvements
- 💾 Installation script changed backup file suffix to .bak for easier recovery
- 📱 WebUI adapted KernelSU API 2.1.1 to support KernelSU 2.1.2+ app name and icon display
- 🎨 Adapted KernelSU WebUI immersion standard
- 🌐 WebUI used fetch API instead of curl command for network requests, improving compatibility
- 🏗️ Modular architecture refactored Zygisk module core
- 🎨 Optimized WebUI display and layout
- 🏗️ Refactored WebUI template page to component-based structure
- 🔄 Migrated C++ atexit implementation to Rust implementation
- 📋 WebUI added configuration metadata support

## ⬆️[v1.0.5 → v1.1.0] - 2025-11-09

### 🆕 New Features
- 🌐 Added WebUI interface for more friendly configuration management experience
- ⚙️ Installation script added configuration selection feature, supporting personalized installation options

### ⚡ Improvements
- 🗺️ Optimized configuration field property mapping logic, improving configuration processing efficiency
- 📦 Migrated to new zygisk-api-rs library and updated dependencies, compatible with more Zygisk implementations
- 🔧 Updated Rust edition to 2024 version, utilizing latest language features
- 📚 Optimized configuration documentation structure, improving readability and usability

## ⬆️[v1.0.0 → v1.0.5] - 2025-11-04

### 🆕 New Features
- 📱 Added device template configuration for convenient application to multiple package names
- 🔄 Added lite and full dual-mode configuration to meet different usage scenarios
- 💾 Added module installation backup configuration mechanism for improved stability

### ⚡ Improvements
- 🚀 Optimized core performance and size for smoother operation
- 🧩 Optimized module unloading logic, reducing memory residency time
- 🎯 Used local_cxa_atexit_finalize_impl to enhance stealth
