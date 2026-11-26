**Türkçe** | [简体中文](https://github.com/uamind/DevMask/blob/main/docs/README.md) | [English](https://github.com/uamind/DevMask/blob/main/docs/en/README.md)

# DevMask 📱

<img src="../logo.png" style="width: 96px;" alt="logo">

**Her uygulama, kendi kimliği.** Paket bazında model, Build parmak izi, sistem özellikleri ve DPI sahtelenir; her uygulama kendi çıkış proxy'sini kullanabilir (kimlik doğrulamalı konut proxy'leri dahil).

> v0.0.1 DevMask'in ilk sürümüdür; sürüm numaralandırması sıfırdan başlar. Geçmiş: [CHANGELOG](CHANGELOG.md).

DevMask **bağımsız olarak geliştirilen bir projedir**; üst akış deposunun fork'u veya aynası değildir. İlk sürümler [Device Faker](https://github.com/Seyud/device_faker) üzerine kuruldu; yapılandırma modeli ve Zygisk enjeksiyonu yaklaşımı oradan gelir. Seyud'a ve topluluğa teşekkürler — özgün telif ve lisans bildirimleri [NOTICES](../../NOTICES) dosyasında ve her kaynak dosyanın başlığında korunur.

## Resmî kaynaklar

| Amaç | Adres |
| --- | --- |
| Depo | [uamind/DevMask](https://github.com/uamind/DevMask) |
| İndirme / güncelleme | [Releases](https://github.com/uamind/DevMask/releases/latest) · [Update.json](../../Update.json) |
| Cihaz şablon deposu (bizim) | [uamind/devmask-config](https://github.com/uamind/devmask-config) |
| Belgeler | [uamind.github.io/devmask](https://uamind.github.io/devmask/) |
| Sorunlar | [uamind/DevMask/issues](https://github.com/uamind/DevMask/issues) |

## Ne çözer

Tek telefonda on uygulama model, Build parmak izi, özellikler ve DPI üzerinden birbiriyle ilişkilendirilebilir; cihaz değişse bile aynı çıkış IP'si paylaşılır. DevMask bunu üç katmanda çözer:

- **Cihaz katmanı** — paket bazında model ve sistem özelliği taklidi; yapılandırılmamış uygulamalar gerçek değerleri okur
- **Ağ katmanı** — cihaz veya uygulama bazında çıkış proxy'si; kimlik doğrulamalı proxy'ler cihaz içi aktarıcı tarafından doldurulur
- **Filo katmanı** — Relay birçok cihazı toplar, modeli toplu uygular ve AI için tek API sunar

## Giriş noktaları

1. **Cihaz WebUI** — root yöneticisinden açın, şablonları ve uygulamaları yapılandırın
2. **Manager / Relay** — tüm cihazları tek yerden yönetin
3. **AI / betikler** — /api/v1 ve /relay/api/v1, HTTP + Bearer

## Hızlı başlangıç

1. Modülü kurun (ZygiskNext gibi bir Zygisk uygulaması gerekir)
2. WebUI'de şablon oluşturun veya tek uygulama için alanları düzenleyin
3. Şablonu pakete bağlayın ve uygulamayı yeniden başlatın
4. Çıkış proxy'si için "Ağ" sekmesini açıp proxy'yi yapılandırın; kimlik doğrulamalı proxy'ler cihaz içi aktarıcıdan geçer
5. Çok cihaz için Manager'ı başlatın, cihaz ekleyin, proxy havuzu ve toplu uygulama kullanın

Yapılandırma: /data/adb/devmask/config/config.toml — [CONFIG.md](CONFIG.md) ve [SERVER.md](SERVER.md).

## Bilinen sınırlar

- Kimlik doğrulamalı proxy'ler cihaz içi aktarıcı gerektirir; cihaz düzeyinde yalnızca HTTP desteklenir
- Uygulama düzeyinde enjeksiyon yalnızca Java ağ yığınını kapsar
- Kesin uid atfı henüz yok (çekirdek owner match + sınırlı yeniden doğrulama)
- Pixel 9 / Android 17 / KernelSU üzerinde doğrulandı

## Lisans

GPL-3.0-or-later. Yeniden dağıtımda SPDX kimlikleri, telif satırları ve lisans bildirimleri korunmalı ve tam kaynak sağlanmalıdır.
