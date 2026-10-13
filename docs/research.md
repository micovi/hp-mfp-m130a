# Research: making the HP LaserJet Pro MFP M130a wireless with a Pico 2 W

Collected 2026-10-13. Confidence levels are the researchers' own; "confirmed" means seen on the
real printer.

## The printer

- **USB identity (confirmed):** `03f0:622a`, product string "HP LaserJet MFP M129-M134", shared by
  the whole M129-M134 family.
- **Interfaces (confirmed by `just probe`):**

  | IF | class/sub/proto | endpoints (out/in) | role |
  | -- | --------------- | ------------------ | ---- |
  | 0  | 07/01/02 | 0x01 / 0x81 | printer class, bidirectional |
  | 1  | ff/04/01 | 0x07 / 0x87 | HP LEDM (embedded web services) |
  | 2  | ff/04/01 | 0x09 / 0x89 | HP LEDM |
  | 3  | ff/09/01 | 0x0b / 0x8b | IPP-over-USB, HP's vendor-class variant |
  | 4  | ff/09/01 | 0x0c / 0x8c | IPP-over-USB, HP variant |
  | 5  | ff/02/01 | 0x03 / 0x83 | HP SOAP scan |

  There is no standard `07/01/04` IPP-over-USB interface. OpenPrinting ipp-usb accepts HP's
  `ff/09/01` as IPP-over-USB for vendor `0x03f0`.
- **macOS already uses IPP-over-USB with it (confirmed):** its CUPS queue for the printer has the
  device URI `ippusb://HP LaserJet MFP M130a (000001)._ipp._tcp.local./?uuid=...`. So the printer
  answers IPP on its USB IPP interfaces. While that queue exists, macOS holds those interfaces.
- **Print languages:** PCLmS, URF, PWG raster (HP datasheet). No PCL5/6, no PostScript. Over the
  printer-class interface it is a host-rendered raster printer; HPLIP drives it with hpcups'
  open-source Hbpl1/PCLm encoder (no plugin for printing).
- **Speed:** Hi-Speed device with a Device Qualifier descriptor, so it should fall back to
  full-speed (64-byte bulk packets) behind the Pico's full-speed-only host. Not yet tried.
- **Scanning:** HPLIP uses the SOAP "HorseThief" protocol on IF5 and needs HP's proprietary
  plugin. eSCL over the IPP interfaces is unconfirmed. VueScan supports the M130a on macOS.
- **Job size (estimate):** hpcups renders 600 dpi 8-bit gray, about 35 MB raw per A4 page; RLE
  makes a text page about 0.3-2 MB, an image page 10-35 MB.

## The Pico 2 W

- **USB host:** native controller and Pico-PIO-USB are both full-speed only (12 Mbps, about
  0.6-1 MB/s of bulk data in practice). Use TinyUSB 0.21.0 or later: it reworked the RP2040/RP2350
  host driver and fixed bulk-transfer bugs (PR #3561, RP2350 DPRAM hard fault). Open issues:
  #3874 (host panic on RX timeout), #4174 (HOST_RESUME IRQ).
- **No printer host class in TinyUSB:** use `CFG_TUH_API_EDPT_XFER` with `tuh_edpt_open()` and
  `tuh_edpt_xfer()` on the bulk endpoints (see TinyUSB's `bare_api` example).
- **USB host and WiFi together:** native USB host does not use PIO; CYW43 uses PIO + DMA for its
  SPI link, so C pico-sdk + TinyUSB (native host) + lwIP is the clean stack. PIO-USB alongside WiFi
  has hung boards (arduino-pico #1336). MicroPython has no USB host; CircuitPython's PIO host has
  composite-device issues on RP2350; Rust/embassy host support is experimental.
- **Wiring:** micro-USB OTG adapter to a USB-A cable; feed 5 V into VBUS (pin 40). The printer
  needs VBUS to attach even though it is self-powered. Debug over UART, since the USB port is
  taken.
- **Throughput:** Pico W WiFi with lwIP in C is about 8 Mbps (CYW43 SPI bound), about the same as
  the USB side. Stream with TCP flow control (`tcp_recved()` after each USB transfer completes);
  a 32-128 KB ring buffer is enough. 520 KB SRAM, no PSRAM.
- **Prior art:** none on the Pico. On the ESP32-S3: LoveLH/ESP32-S3-USB-PrintServer (port 9100,
  HP workarounds), WARMW00D/ESP32-S3-PrintServer (9100 + LPD), MAXXTANG/ESP32S3_PrintServer,
  pldrobot/ESP32S3-USBIP-Server (USB/IP, scanning claimed).

## Laptop side and architectures

- **A. IPP-over-USB proxy on the Pico (preferred):** the Pico forwards HTTP between WiFi and
  IF3/IF4 and advertises `_ipp._tcp` (subtype `_universal`) and `_uscan._tcp` over mDNS (lwIP's
  responder, `LWIP_MDNS_RESPONDER=1`). macOS sees an AirPrint printer (and AirScan scanner, if eSCL
  works) with no driver. The proxy must parse HTTP framing and drain each response completely, or
  a client that disconnects mid-response leaves bytes that corrupt the next one (ipp-usb's
  lesson). Roughly 1-2k lines of C. ipp-usb's quirks: a similar HP (M28-M31) needs
  `Connection: keep-alive`.
- **B. Port 9100 bridge:** the Pico copies TCP to IF0. The Mac needs a driver that emits PCLmS;
  HP ships no current Mac driver for this model, so the laptop would render PCLm itself. Printing
  only.
- **C. Raspberry Pi Zero 2 W:** `ipp-usb` (or CUPS + HPLIP) just works for printing. Scanning via
  HPLIP needs the x86-only-historically plugin.
- **USB/IP: not viable on macOS.** The only client (usbip-macos) is experimental and needs SIP
  disabled; VirtualHere's server does not run on microcontrollers.

## Sources

- HP datasheet: https://www.laserexpressinc.com/manuals/HP/hp-lj-pro-m130-datasheet.pdf
- HP specifications: https://support.hp.com/ie-en/document/c05228178
- lsusb dump: https://linux-hardware.org/?id=usb:03f0-622a
- HPLIP models and PPD: https://raw.githubusercontent.com/mwilck/hplip/master/data/models/models.dat,
  https://raw.githubusercontent.com/mwilck/hplip/master/ppd/hpcups/hp-laserjet_mfp_m129-m134.ppd
- HPLIP scan: https://raw.githubusercontent.com/mwilck/hplip/master/scan/sane/soapht.c
- ipp-usb: https://github.com/OpenPrinting/ipp-usb (HP class check in `usbcommon.go`, quirks in
  `ipp-usb-quirks/`)
- TinyUSB: https://github.com/hathach/tinyusb, release 0.21.0, issues #3533, #3874, #4174
- arduino-pico PIO-USB + WiFi: https://github.com/earlephilhower/arduino-pico/issues/1336
- CircuitPython usb_host on RP2350: https://github.com/adafruit/circuitpython/issues/10760
- lwIP mDNS: https://www.nongnu.org/lwip/2_0_x/group__mdns.html
- ESP32-S3 print servers: https://github.com/LoveLH/ESP32-S3-USB-PrintServer,
  https://github.com/WARMW00D/ESP32-S3-PrintServer, https://github.com/MAXXTANG/ESP32S3_PrintServer,
  https://github.com/pldrobot/ESP32S3-USBIP-Server
- usbip-macos: https://github.com/carlossless/usbip-macos
- VueScan M130a: https://www.hamrick.com/vuescan/hp_laserjet_pro_m130a.html
