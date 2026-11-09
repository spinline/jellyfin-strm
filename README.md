# Jellyfin STRM Generator (Rust)

Ultra-lightweight, zero-overhead Rust daemon that synchronizes remote Jellyfin media libraries into local `.strm` files for direct playback on any media player (Kodi, VLC, Infuse, Emby, Plex, Fygos NAS web player).

## Özellikler

- **Doğrudan Akış (Direct Streaming):** Yerel bir FUSE dosya sistemi veya aracı sunucuya ihtiyaç duymaz. Oluşturulan `.strm` dosyaları Jellyfin'in doğrudan akış bağlantısını (`stream?static=true`) içerir. Oynatıcı dosyayı açtığında doğrudan Jellyfin sunucusundan akış başlar.
- **Her Film Kendi Klasöründe:** Standart medya yöneticisi uyumluluğu için filmler `[Film Adı (Yıl)]` klasörleri altında toplanır.
- **Farklı Çözünürlük ve Sürümler:** Bir filmin birden fazla çözünürlüğü veya versiyonu (4K, 1080p, Remux vb.) varsa, Jellyfin'deki gerçek dosya adıyla `[gerçek_dosya_adı].strm` olarak oluşturulur.
- **Diziler ve Sezonlar:** Dizi kütüphaneleri `[Dizi Adı]/[Sezon]/[bölüm_adı].strm` yapısında organize edilir.
- **Otomatik API Anahtarı ve Giriş:** Kullanıcı adı ve şifreyle Jellyfin'e güvenli şekilde giriş yapar, oturum anahtarını otomatik yönetir.
- **Periyodik Tarama (Scheduler):** Yapılandırılan zaman aralığında (örneğin 6 saatte bir) kütüphaneyi tarar, sadece yeni veya güncellenen dosyaları yazar.
- **Ultra Düşük Kaynak Tüketimi:** Rust ile yazılmıştır; ~15 MB RAM tüketir ve CPU yükü yaratmaz.

## Docker ile Kurulum

`docker-compose.yml` hazır olarak sunulmuştur:

```yaml
services:
  jellyfin-strm:
    image: jellyfin-strm:latest
    build: .
    container_name: jellyfin-strm
    restart: unless-stopped
    environment:
      - JELLYFIN_URL=https://jellyfin.example.com
      - JELLYFIN_USERNAME=kullanici_adi
      - JELLYFIN_PASSWORD=parola
      - OUTPUT_DIR=/media
      - SCAN_INTERVAL_MINUTES=360
      - CLEAN_DELETED=true
      - INCLUDE_MOVIES=true
      - INCLUDE_SERIES=true
    volumes:
      - /vol1/1000/Docker/strm:/media
```

## Yapılandırma Seçenekleri (Environment Variables)

| Değişken | Varsayılan | Açıklama |
|---|---|---|
| `JELLYFIN_URL` | - | Uzak Jellyfin sunucu adresi (örn: `https://jellyfin.example.com`) |
| `JELLYFIN_USERNAME` | - | Jellyfin kullanıcı adı |
| `JELLYFIN_PASSWORD` | - | Jellyfin kullanıcı parolası |
| `JELLYFIN_API_KEY` | - | Varsa statik API anahtarı (kullanıcı adı/parola yerine kullanılabilir) |
| `OUTPUT_DIR` | `/media` | `.strm` dosyalarının yazılacağı hedef dizin |
| `SCAN_INTERVAL_MINUTES`| `360` (6 saat) | Tarama sıklığı (dakika). `0` verilirse tek sefer çalışıp çıkar |
| `CLEAN_DELETED` | `true` | Jellyfin'den silinen medyaların `.strm` dosyalarını temizler |
| `INCLUDE_MOVIES` | `true` | Film kütüphanelerini senkronize eder |
| `INCLUDE_SERIES` | `true` | Dizi kütüphanelerini senkronize eder |
