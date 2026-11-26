# DevMask 📱

**One app, one identity.** DevMask spoofs model, Build fingerprint, system properties and DPI per package, and lets each app egress through its own proxy. Apps without a config keep reading the real values.

## What it solves

Ten apps on one phone can be linked through model, Build, properties and DPI; switch devices and they usually still share one home IP. DevMask splits the two problems:

- **Device layer** — per-package model and property spoofing; only configured apps are affected
- **Network layer** — a device-wide or per-app egress proxy; authenticated proxies are filled in by an on-device relay
- **Fleet layer** — a Manager / Relay aggregates many devices, applies one model in bulk and exposes a single API

## Capabilities

| Area | Details |
| --- | --- |
| Model / templates | manufacturer, brand, model, device, product, board, SoC, fingerprint, build id — applied as templates to many packages |
| System properties | property-family expansion (including ro.product partition copies), custom-property allow list, __DELETE__ / __EMPTY__ semantics |
| Version and DPI | Android version, SDK, security patch, Build.DISPLAY, incremental; per-app display density (120–640) |
| Per-app egress | HTTP or SOCKS5 proxy per package without affecting other apps |
| Authenticated relay | credentials filled by an on-device relay; HTTP Basic and SOCKS5 (RFC 1929) |
| Fleet and audit | Manager proxy pool, bindings, job dispatch, per-device results, exit-IP checks, audit log |

## Quick start

1. Install the module (a Zygisk implementation such as ZygiskNext is required)
2. Open the WebUI, create a model template under **Templates**, or edit a single app
3. Bind the template to a package under **Apps**, then restart that app
4. For egress, fill in a proxy in Settings; authenticated proxies go through the on-device relay automatically
5. For multiple devices, start the Manager, add devices (address + device token), then use the proxy pool and bulk apply

## WebUI

- **Overview** — module version, masked app count, templates, egress state and shortcuts
- **Templates** — create, edit, import/export, plus an online template library
- **Apps** — per-package identities; app clones can inherit or override their own config
- **Settings** — theme / language, egress, debug switches and API access

See the [WebUI guide](./webui.md), [configuration](./configuration/index.md) and [FAQ](./faq.md).

## Online template library

The library is backed by the community repository [device_faker_config](https://github.com/Seyud/device_faker_config); entries can be previewed and applied from the WebUI.

## Acknowledgments 🙏

- [zygisk-dump-dex](https://github.com/ri-char/zygisk-dump-dex) — prototype reference for a Rust Zygisk module
- [zygisk-api-rs](https://github.com/rmnscnce/zygisk-api-rs) — Rust bindings for the Zygisk API
- [MiPushZygisk](https://github.com/wushidia/MiPushZygisk) — reference for Zygisk device spoofing

---

**📱 Let your device stop being limited by app model checks.** 🚀
