# WebUI

DevMask provides a modern web-based management interface. Open it directly from the module page of WeaveMask / KernelSU / APatch — no need to hand-edit TOML.

## How to Open

After installing the module, find **DevMask** in your module manager and tap the "WebUI" or "Settings" entry.

::: info Compatibility
The WebUI is implemented on the pure KernelSU WebUI API. It works with the module WebUI entry points of KernelSU, WeaveMask and APatch.
:::

## Layout

Five bottom tabs, always in this order: **Overview / Templates / Apps / Network / Settings**. You can also swipe left and right between pages. The theme follows the system by default; light and dark share one skeleton with different tokens.

| Page | Main Features |
|------|---------------|
| **Overview** | Device line, configured app count, four metrics (templates / egress / module version / last saved), favorite templates, recent changes |
| **Templates** | Create, edit, delete, search device templates; online template library; import/export; attach packages in bulk |
| **Apps** | Installed app list with configured / all / system filters and search, plus per-app spoofing config |
| **Network** | Device-wide egress: status, proxy definitions, exit-IP probe, on-device relay, and the always-available "Go direct" action |
| **Settings** | Appearance (theme and language), Advanced (module settings / tools / external access / service status), About |

## Overview

- **Header**: in native service mode it shows the device manufacturer and model plus the Android version; in manager mode a single line explains what the current environment can do
- **Hero**: how many apps are currently configured, with two entries — "Manage apps" jumps to the Apps page, "New template" jumps to Templates
- **Four metrics**: template count, egress state (connected / direct / disabled), module version, last saved time
- **Favorite templates**: the top 3 templates by package count. A card title prefers the market name, then the model, then the template name; the model appears as a subtitle only when both market name and model exist, and "N apps" is how many packages the template carries. "All N →" opens the Templates page. When only the template name is left, the title is truncated to one line instead of breaking the card
- **Recent changes**: config save time and egress connection state; an empty-state line appears when there is nothing to show

## App Management

### App List and Filtering

- Lists installed apps and shows whether each one is currently configured
- The header shows a configured / total counter across all visible apps; it does not change with search or filters
- Three filter pills: **Configured** / **All** (default) / **System apps**, each with a count (the system count appears once that list is read) — switch to "System apps" to configure system apps too
- Search by app label or package name; search and pills combine

### Multi-user and Cloned Apps

Multi-user instances of the same app (e.g. `com.foo` and `com.foo@999`) are deduplicated **per instance**: `com.foo` and `com.foo@0` are the same instance and no longer show up as two rows.

- A clone that only matches the main app config through the fallback shows a blue **"Inherited from main app"** badge instead of "Configured", so you never mistake it for having its own config
- The config dialog states the scope being edited: main app only (user 0) / main app + all clones (fallback) / that clone only (user N)
- Use `com.foo@0` to spoil only the main app and keep clones inheriting nothing; use the bare `com.foo` to cover the main app and all its clones
- A "Clones and inheritance" note card at the bottom of the list appears only when clones actually follow a main app, and states how many there are

### Editing App Config

Tap an app to enter the config editor:

- Fill in fields directly (manufacturer, brand, model, device codename, etc.)
- Apply an existing [device template](./configuration/index.md#method-one-device-templates) in one click
- Set toggles such as `companion_resetprop`, `force_denylist_unmount`, `dpi`
- Add custom properties in the `custom_props` section

### Applying a Template to an App

In the app editor, choose "Apply template" to populate all fields from a template at once, then tweak as needed.

## Template Management

### Header and Search

The first header row holds the "Device templates" title, the "N templates" count and the primary "New template" action; the second row holds the search field plus "Online templates" and "Import/Export". A template card shows only three things — the template name, the model code (`model` / `device` / `product`, or "No model set" when all are empty) and how many apps use it; every other field lives in the edit dialog.

### Create and Edit Templates

- Fill in device fields and save as a named template
- Saved templates can be reused across multiple apps

### Apply to Multiple Packages

When editing a template, add package names to its `packages` list. Those packages will automatically use the template once saved.

::: warning One package, one template
When several templates declare the same package, only the one written **first** in `config.toml` wins at runtime. The Templates page shows a collapsed "N duplicate package configs" banner listing which template takes effect and which is ignored once expanded; deduplicate package names before merging templates.
:::

### Import / Export

- **Export**: tick the templates to export (multi-select), generate TOML text and copy it in one click
- **Import**: paste TOML text to import templates
- **Read this device**: run `getprop` and turn the current device properties into a template you can save or copy

### Online Template Library

The WebUI has a built-in online template source. Filter community templates, then import them in one click:

- Repository: [uamind/devmask-config](https://github.com/uamind/devmask-config) (maintained by DevMask)
- The library first shows the data source and detail-fetch progress, with refresh and retry for failed items
- Category chips (All / Common / Gaming / Transcend) combine with brand chips
- Search by brand, model, template name or **template description**; space-separated keywords must all match
- Tap any row to open the preview drawer, check fingerprint, author, version and description, then import. The back gesture closes the drawer first, then the library

::: tip Contribute
If you have a well-organized device config, submit it to `uamind/devmask-config` to help others.
:::

### Rename Existing Templates

Templates can be renamed. After renaming, all app configs that reference the template are automatically updated.

## Network

The Network page is the **only** place to configure the device-side egress (Settings keeps just an entry card). The status card and the "Go direct" button stay at the top; proxy definitions, probe results and the on-device relay follow.

### Egress Status and Go Direct

- State pill: **Connected** / **Direct** / **Disabled**; "Degraded to direct" is appended while fail-open is in effect, "Not applied (readback differs)" when the read-back disagrees
- The status card reports desired mode, effective mode, whether the system proxy is applied, the settings read-back, `egress revision`, `egress.toml`, credential state and the last probe; "Refresh" re-reads everything
- **Go direct**: always in the header whatever the current state; after a confirmation it clears the system proxy and falls back to a direct connection

### Proxy Definitions

- Each entry shows name, protocol (HTTP / SOCKS5 …), `host:port`, whether credentials are stored, and the actual path (through the on-device relay at `127.0.0.1:port`, straight to upstream, or credentials not in effect)
- "Edit" opens the egress dialog: enable the device-level proxy, pick a type, set host and port, username and password (stored credentials can be cleared)
- The dialog's "Advanced" group holds the probe URL, timeout, fail-open and probe target

### Probe Results (exit IP)

- The card header shows the current probe target; "Edit" jumps straight to the advanced group
- "Probe exit IP" reports the exit IP, latency, path, probe time, upstream status and scope; a result judged approximate is tagged and explained
- When the current environment cannot probe, the button is disabled and the reason is shown

### On-device Relay

- Shows whether the relay is enabled, its base port, fail-open, concurrency limits, uid firewall state and the listener list (which proxy listens on which port)
- "Restart relay" is only enabled when the service supports it; when relay ports change, the card lists the apps that need restarting
- Proxies with credentials are held by the on-device relay: apps only connect to `127.0.0.1:port`, so credentials never reach app configs

## Settings

Settings is split into **Appearance / Advanced / About**, with a "Network egress" entry card in between.

### Appearance

- **Theme**: follow system (default) / light / dark
- **Language**: follow system / 简体中文 / English / Türkçe

### Network Egress Entry

A full-width row that opens the Network page. Egress forms, probing and the relay are not duplicated in Settings.

### Advanced

- **Module settings**: debug mode (the `config.toml` `debug` switch; Info-level records only reach the log file when it is on); the log path `/data/adb/devmask/logs/devmask.log` sits below it and can be copied in one tap
- **Tools → Config conversion**: pick a zip from another device-spoofing module; the WebUI parses its `system.prop` inside the WebView and converts it into a template (nothing written to disk, no CLI calls). Review the preview, then "Save as template". Handy when migrating from an older module
- **External access**: let other devices on the LAN reach the WebUI, with a configurable port and access token, plus the bind address, status and LAN addresses. Save and restart the service, reset to the default token, refresh the status
- **Service status** (card title "Server"): transport (manager mode / native service / read-only), server version, bind address, config revision, last modified, whether a token is set, and scopes; refresh, set a token, sign out

::: tip Read-only environments
When the native service is unreachable (`/api/v1/healthz` fails), the page goes read-only: the external access card is hidden, service status keeps only the readable fields plus "Refresh", and every write action is hidden.
:::

### About

Version (read from `module.prop`, falling back to the WebUI's own version before it is ready), author, repository, follow the module, translators and license (`SPDX-License-Identifier: GPL-3.0-or-later`, tapping opens the LICENSE). Repository, follow and translators each open their dialog or external link.

## Gestures and Navigation

- **Swipe back**: swipe from the edge to go back one level in multi-level pages (app → editor → field details, template library → preview drawer)
- **Swipe between tabs**: quickly switch between the five bottom tabs with left/right swipes
- **Back button**: the online library and its preview drawer hook into history, so back closes the drawer first, then the library, and only then leaves the Templates page

## Multiple Languages

The WebUI supports language switching. Currently available:

- 简体中文
- English
- Türkçe

The UI language follows the system language by default, and can also be switched manually in the WebUI Settings page. Under Turkish, page titles scale down with screen width (16px at ≤384px), so 360dp keeps one line; screens at ≤320dp may still wrap.

## Next Steps

- [Basic Configuration](./configuration/index.md) — understand the TOML fields behind the WebUI
- [FAQ](./faq.md) — check here first if something in the WebUI does not work
