**English** | [简体中文](https://github.com/uamind/DevMask/blob/main/docs/README.md) | [Türkçe](https://github.com/uamind/DevMask/blob/main/docs/tr/README.md)

# DevMask 📱

<img src="../logo.png" style="width: 96px;" alt="logo">

**One app, one identity.** Per-package model, Build fingerprint, system properties and DPI — and each app can egress through its own proxy, authenticated residential ones included.

> v0.0.1 is the first DevMask release; versioning restarts from zero. History: [CHANGELOG](CHANGELOG.md).

## What it solves

Ten apps on one phone can be linked through model, Build fingerprint, properties and DPI; switch devices and they still share one home IP. DevMask splits the problem:

- **Device layer** — per-package spoofing of model and system properties; apps without config still read the real values
- **Network layer** — device-wide or per-app egress proxy; authenticated proxies are filled in by an on-device relay
- **Fleet layer** — a Relay aggregates many devices, applies one model in bulk, and exposes a single API for AI agents

## Capabilities

| Area | Details |
| --- | --- |
| Model / templates | manufacturer, brand, model, device, product, board, SoC, fingerprint, build id, and more; apply as templates to many packages |
| System properties | property-family expansion (including ro.product partition copies), custom-property allow list, __DELETE__ / __EMPTY__ semantics |
| Version spoofing | Android version, SDK, security patch, Build.DISPLAY, incremental |
| DPI | per-app display density (120–640) |
| Per-app egress | HTTP or SOCKS5 proxy per package without affecting other apps |
| Authenticated relay | credentials filled by an on-device relay; HTTP Basic and SOCKS5 (RFC 1929) |
| Device egress | device-wide HTTP proxy through system settings |
| Fleet tooling | Manager proxy pool, bindings, job dispatch, per-device results, exit-IP checks, audit |
| Observability | status / overview / templates / app matrix / egress probe / audit log |

## Entry points

1. **Device WebUI** — open it from your root manager, configure templates and apps, enable external access for LAN use
2. **Manager / Relay** — manage every device from one place: unified models, proxy pool, bulk apply
3. **AI / scripts** — device /api/v1 and manager /relay/api/v1, both HTTP + Bearer; see [API.md](API.md) and [manager/docs/API.md](../../manager/docs/API.md)

## Quick start

1. Install the module (requires a Zygisk implementation such as ZygiskNext; the built-in Magisk Zygisk is not supported)
2. Open the WebUI and create a model template, or edit fields for a single app
3. Bind the template to a package and restart that app
4. For egress, fill in a proxy on the "Network egress" card; authenticated proxies go through the on-device relay automatically
5. For many devices, start the Manager, add devices (base URL + device token), then use the proxy pool and bulk apply

Config lives at /data/adb/devmask/config/config.toml — see [CONFIG.md](CONFIG.md) and [SERVER.md](SERVER.md).

## Known limits

- Authenticated proxies require the on-device relay (built in since v0.0.1); device-level egress supports HTTP only, not SOCKS5
- App-level injection covers the Java networking stack only; native and WebView traffic is unaffected, and configured apps keep the module code pages mapped
- Strict uid attribution is still pending (kernel owner match plus rate-limited re-checks today)
- Validated on Pixel 9 / Android 17 / KernelSU; other devices need their own verification

## Docs

- [Configuration](CONFIG.md) · [Service & API](SERVER.md) · [API contract](API.md)
- Manager: [API](../../manager/docs/API.md) · [Architecture](../../manager/docs/ARCH.md) · [Runbook](../../manager/docs/RUN.md)

## Credits

Early versions were built on [Device Faker](https://github.com/Seyud/device_faker) by Seyud; the configuration model and Zygisk injection approach originate there. Original copyright and license notices are preserved in [NOTICES](../../NOTICES) and in every source file header.

## License

GPL-3.0-or-later. Redistributions must keep the SPDX identifiers, copyright lines and license notices, and provide the complete corresponding source.
