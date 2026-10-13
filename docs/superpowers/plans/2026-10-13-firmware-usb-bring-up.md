# Firmware milestone 1: the Pico 2 W talks IPP to the printer over USB

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Firmware for the Pico 2 W that enumerates the HP M130a on its USB port, sends the same IPP
Get-Printer-Attributes and eSCL ScannerCapabilities requests that `just probe` sends from the Mac,
and logs the answers on UART0. This proves the USB half of the bridge (full-speed fallback, the
`ff/09/01` interface, zero-length-packet handling) before any WiFi work.

**Architecture:** C on pico-sdk 2.3.1 with TinyUSB 0.21.0 in host mode on the RP2350's native USB
port (pico-sdk bundles TinyUSB 0.18, which predates the RP2350 host fixes, so the flake pins 0.21.0
and points `PICO_TINYUSB_PATH` at it). The hardware-independent parts (configuration-descriptor
parsing, request encoding, HTTP response framing) are plain C11 with host unit tests run against
descriptors captured from the real printer. `printer_link.c` drives TinyUSB's asynchronous endpoint
API from the main loop; there is no RTOS.

**Tech Stack:** pico-sdk 2.3.1, TinyUSB 0.21.0, arm-none-eabi-gcc 15.3, CMake + Ninja, probe-rs
(flashing through the Debug Probe), tio (UART log), all from `flake.nix`.

**Background:** [docs/research.md](../../research.md) has the printer facts this plan relies on
(interfaces, the zero-length packets idle IPP endpoints return, the full-speed descriptors).
Wiring: [docs/wiring.svg](../../wiring.svg).

**Already verified while writing this plan** (on 2026-10-13, macOS 27, aarch64-darwin): every
source file below compiles for `pico2_w` with `-Wall -Wextra -Werror` and zero warnings (45.6 KB
text, 6.2 KB bss); the host tests pass with UBSan under nixpkgs' clang 21. Nothing has run on a Pico
yet. The captured descriptors are already in `firmware/tests/fixtures/`.

---

## File structure

```
firmware/
  CMakeLists.txt          Pico build: pico_stdlib + tinyusb_host, stdio on UART0
  src/
    tusb_config.h         TinyUSB host config (raw endpoint API, one device, no hub)
    usb_desc.{h,c}        find a class/subclass/protocol interface's bulk OUT+IN pair   [host-tested]
    http_request.{h,c}    HTTP request heads + IPP Get-Printer-Attributes body          [host-tested]
    http_framer.{h,c}     where one HTTP/1.1 response ends in a byte stream             [host-tested]
    printer_link.{h,c}    TinyUSB mount callbacks, endpoint setup, one exchange at a time
    main.c                milestone-1 app: run the two probes once per mount, log results
  tests/
    CMakeLists.txt        host build of the tested modules with UBSan, run by ctest
    check.h               CHECK macro (no test framework dependency)
    test_usb_desc.c, test_http_request.c, test_http_framer.c
    fixtures/m130a-config-{high,full}-speed.bin   captured from the printer (already present)
```

`printer_link.c` and `main.c` only build for the Pico; everything they need from protocol logic
lives in the host-tested modules.

---

### Task 1: Firmware toolchain in the Nix dev shell

**Files:**
- Modify: `flake.nix`
- Modify: `.gitignore`

- [ ] **Step 1: Replace `flake.nix`**

```nix
{
  description = "hp-mfp-m130a: every tool the justfile needs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    # pico-sdk 2.3.1 bundles TinyUSB 0.18; the RP2350 host fixes are in 0.21.0.
    tinyusb = {
      url = "github:hathach/tinyusb/0.21.0";
      flake = false;
    };
  };

  outputs =
    { nixpkgs, tinyusb, ... }:
    let
      # nixpkgs-unstable no longer builds for Intel Macs (x86_64-darwin).
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forEachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forEachSystem (
        pkgs:
        let
          # With cyw43-driver and lwIP, which the WiFi milestone needs.
          picoSdk = pkgs.pico-sdk.override { withSubmodules = true; };
        in
        {
          default = pkgs.mkShell {
            packages = [
              # The USB probe (just probe). nixpkgs' pyusb is patched to load nixpkgs' libusb, so
              # nothing comes from Homebrew or the system Python.
              (pkgs.python313.withPackages (ps: [
                ps.pyusb
                ps.pytest
              ]))
              # Lint, format and type check (just lint).
              pkgs.ruff
              pkgs.ty
              # Runs the recipes in the justfile.
              pkgs.just
              # The firmware (just fw-build, just fw-test).
              pkgs.gcc-arm-embedded
              pkgs.cmake
              pkgs.ninja
              pkgs.picotool
              # Flash through the Debug Probe (just flash) and read the Pico's UART (just logs).
              pkgs.probe-rs-tools
              pkgs.tio
            ];

            PICO_SDK_PATH = "${picoSdk}/lib/pico-sdk";
            PICO_TINYUSB_PATH = "${tinyusb}";
            # pico-sdk builds picotool from source unless it finds this one.
            picotool_DIR = "${pkgs.picotool}/lib/cmake/picotool";
          };
        }
      );
    };
}
```

- [ ] **Step 2: Ignore build output**

Append to `.gitignore`:

```
# CMake build trees (just fw-test, just fw-build).
build/
```

- [ ] **Step 3: Enter the shell and check every tool resolves**

Run:

```bash
git add flake.nix .gitignore && nix develop --command bash -c 'arm-none-eabi-gcc --version | head -1; cmake --version | head -1; ls "$PICO_SDK_PATH/lib"; ls "$PICO_TINYUSB_PATH/hw/bsp/rp2040/family.cmake"; ls "$picotool_DIR"; probe-rs --version; tio --version | head -1'
```

Expected: `arm-none-eabi-gcc (Arm GNU Toolchain 15.3.Rel1 ...)`, `cmake version 4.x`, the pico-sdk
`lib` listing `btstack cyw43-driver lwip mbedtls tinyusb` (submodules present), the TinyUSB
`family.cmake` path, picotool's CMake files, `probe-rs 0.32.0`, `tio v3.9`. `git add` comes first
because flakes only see tracked files.

- [ ] **Step 4: Commit**

```bash
git add flake.nix flake.lock .gitignore
git commit -m "Add the Pico firmware toolchain to the dev shell"
```

---

### Task 2: Host test harness and configuration-descriptor parsing

**Files:**
- Create: `firmware/tests/check.h`, `firmware/tests/CMakeLists.txt`, `firmware/tests/test_usb_desc.c`
- Create: `firmware/src/usb_desc.h`, `firmware/src/usb_desc.c`
- Modify: `justfile`

- [ ] **Step 1: Add the test recipe to `justfile`**

```make
# Host unit tests for the firmware's hardware-independent modules.
fw-test:
    cmake -S firmware/tests -B build/fw-tests -G Ninja
    cmake --build build/fw-tests
    ctest --test-dir build/fw-tests --output-on-failure
```

- [ ] **Step 2: Write the harness**

`firmware/tests/check.h`:

```c
// A minimal test harness: CHECK records a failure and carries on; main returns the verdict.
#ifndef CHECK_H
#define CHECK_H

#include <stdio.h>

static int check_failures;

#define CHECK(condition)                                                                       \
    do {                                                                                       \
        if (!(condition)) {                                                                    \
            fprintf(stderr, "%s:%d: CHECK failed: %s\n", __FILE__, __LINE__, #condition);      \
            check_failures++;                                                                  \
        }                                                                                      \
    } while (0)

#define CHECK_RESULT() (check_failures == 0 ? 0 : 1)

#endif
```

`firmware/tests/CMakeLists.txt`:

```cmake
# Host-side unit tests for the firmware's hardware-independent modules.
cmake_minimum_required(VERSION 3.20)
project(m130a_bridge_tests C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(SRC ${CMAKE_CURRENT_LIST_DIR}/../src)
# UBSan only: ASan's runtime hangs at start-up with nixpkgs' clang on macOS.
add_compile_options(-Wall -Wextra -Werror -fsanitize=undefined -fno-sanitize-recover=undefined)
add_link_options(-fsanitize=undefined)

enable_testing()
foreach(module usb_desc)
    add_executable(test_${module} test_${module}.c ${SRC}/${module}.c)
    target_include_directories(test_${module} PRIVATE ${SRC})
    target_compile_definitions(test_${module} PRIVATE
        FIXTURE_DIR="${CMAKE_CURRENT_LIST_DIR}/fixtures")
    add_test(NAME ${module} COMMAND test_${module})
endforeach()
```

- [ ] **Step 3: Write the header and the failing test**

`firmware/src/usb_desc.h`:

```c
// Finds a vendor interface's bulk endpoint pair in a raw USB configuration descriptor.
#ifndef USB_DESC_H
#define USB_DESC_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct {
    uint8_t interface_number;
    uint8_t ep_out;
    uint8_t ep_in;
    uint16_t ep_out_size;
    uint16_t ep_in_size;
    // Offsets of the two endpoint descriptors in the configuration descriptor, for opening them.
    uint16_t ep_out_offset;
    uint16_t ep_in_offset;
} usb_bulk_pair_t;

// Finds the first interface (alternate setting 0) with the given class, subclass and protocol
// that has both a bulk OUT and a bulk IN endpoint. Returns false if there is none or the
// descriptor is malformed.
bool usb_find_bulk_pair(const uint8_t *config, size_t len, uint8_t cls, uint8_t subclass,
                        uint8_t protocol, usb_bulk_pair_t *out);

#endif
```

`firmware/tests/test_usb_desc.c`:

```c
#include <stdlib.h>
#include <string.h>

#include "check.h"
#include "usb_desc.h"

// Reads a configuration descriptor captured from the real printer (see tests/fixtures/).
static size_t load(const char *name, uint8_t *buf, size_t cap) {
    char path[512];
    snprintf(path, sizeof path, "%s/%s", FIXTURE_DIR, name);
    FILE *file = fopen(path, "rb");
    if (file == NULL) {
        fprintf(stderr, "cannot open fixture %s\n", path);
        exit(2);
    }
    size_t len = fread(buf, 1, cap, file);
    fclose(file);
    return len;
}

static void test_ipp_interface_at_full_speed(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t ipp;
    CHECK(usb_find_bulk_pair(config, len, 0xFF, 0x09, 0x01, &ipp));
    CHECK(ipp.interface_number == 3);
    CHECK(ipp.ep_out == 0x0B && ipp.ep_in == 0x8B);
    CHECK(ipp.ep_out_size == 64 && ipp.ep_in_size == 64);
    CHECK(config[ipp.ep_out_offset + 1] == 5 && config[ipp.ep_out_offset + 2] == 0x0B);
    CHECK(config[ipp.ep_in_offset + 1] == 5 && config[ipp.ep_in_offset + 2] == 0x8B);
}

static void test_ipp_interface_at_high_speed(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-high-speed.bin", config, sizeof config);
    usb_bulk_pair_t ipp;
    CHECK(usb_find_bulk_pair(config, len, 0xFF, 0x09, 0x01, &ipp));
    CHECK(ipp.interface_number == 3);
    CHECK(ipp.ep_out_size == 512 && ipp.ep_in_size == 512);
}

static void test_other_interfaces(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t pair;
    CHECK(usb_find_bulk_pair(config, len, 0x07, 0x01, 0x02, &pair));
    CHECK(pair.interface_number == 0 && pair.ep_out == 0x01 && pair.ep_in == 0x81);
    CHECK(usb_find_bulk_pair(config, len, 0xFF, 0x04, 0x01, &pair));
    CHECK(pair.interface_number == 1 && pair.ep_out == 0x07 && pair.ep_in == 0x87);
    CHECK(!usb_find_bulk_pair(config, len, 0x07, 0x01, 0x04, &pair));
}

static void test_malformed_descriptors(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t pair;
    CHECK(!usb_find_bulk_pair(config, 100, 0xFF, 0x09, 0x01, &pair));
    uint8_t zero_length[512];
    memcpy(zero_length, config, len);
    zero_length[9] = 0;
    CHECK(!usb_find_bulk_pair(zero_length, len, 0xFF, 0x09, 0x01, &pair));
    CHECK(!usb_find_bulk_pair(config, 0, 0xFF, 0x09, 0x01, &pair));
}

int main(void) {
    test_ipp_interface_at_full_speed();
    test_ipp_interface_at_high_speed();
    test_other_interfaces();
    test_malformed_descriptors();
    return CHECK_RESULT();
}
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `just fw-test`
Expected: CMake error `Cannot find source file: .../firmware/src/usb_desc.c`.

- [ ] **Step 5: Implement**

`firmware/src/usb_desc.c`:

```c
#include "usb_desc.h"

#include <string.h>

enum {
    DESC_INTERFACE = 4,
    DESC_ENDPOINT = 5,
    INTERFACE_LEN = 9,
    ENDPOINT_LEN = 7,
    ENDPOINT_DIR_IN = 0x80,
    ENDPOINT_TYPE_MASK = 0x03,
    ENDPOINT_TYPE_BULK = 0x02,
};

static bool matches(const uint8_t *itf, uint8_t cls, uint8_t subclass, uint8_t protocol) {
    return itf[3] == 0 && itf[5] == cls && itf[6] == subclass && itf[7] == protocol;
}

static void note_endpoint(const uint8_t *ep, size_t offset, usb_bulk_pair_t *pair) {
    if ((ep[3] & ENDPOINT_TYPE_MASK) != ENDPOINT_TYPE_BULK) {
        return;
    }
    uint16_t size = (uint16_t)(ep[4] | ep[5] << 8);
    if ((ep[2] & ENDPOINT_DIR_IN) != 0 && pair->ep_in == 0) {
        pair->ep_in = ep[2];
        pair->ep_in_size = size;
        pair->ep_in_offset = (uint16_t)offset;
    } else if ((ep[2] & ENDPOINT_DIR_IN) == 0 && pair->ep_out == 0) {
        pair->ep_out = ep[2];
        pair->ep_out_size = size;
        pair->ep_out_offset = (uint16_t)offset;
    }
}

bool usb_find_bulk_pair(const uint8_t *config, size_t len, uint8_t cls, uint8_t subclass,
                        uint8_t protocol, usb_bulk_pair_t *out) {
    usb_bulk_pair_t pair = {0};
    bool inside = false;
    for (size_t offset = 0; offset + 2 <= len;) {
        const uint8_t *desc = config + offset;
        if (desc[0] < 2 || offset + desc[0] > len) {
            return false;
        }
        if (desc[1] == DESC_INTERFACE && desc[0] >= INTERFACE_LEN) {
            if (inside && pair.ep_out != 0 && pair.ep_in != 0) {
                break;
            }
            inside = matches(desc, cls, subclass, protocol);
            memset(&pair, 0, sizeof pair);
            pair.interface_number = desc[2];
        } else if (inside && desc[1] == DESC_ENDPOINT && desc[0] >= ENDPOINT_LEN) {
            note_endpoint(desc, offset, &pair);
        }
        offset += desc[0];
    }
    if (!inside || pair.ep_out == 0 || pair.ep_in == 0) {
        return false;
    }
    *out = pair;
    return true;
}
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `just fw-test`
Expected: `100% tests passed, 0 tests failed out of 1`, no compiler warnings.

- [ ] **Step 7: Commit**

```bash
git add justfile firmware/tests firmware/src/usb_desc.h firmware/src/usb_desc.c
git commit -m "Find the printer's IPP-over-USB endpoints in its config descriptor"
```

---

### Task 3: Request encoding

**Files:**
- Create: `firmware/src/http_request.h`, `firmware/src/http_request.c`, `firmware/tests/test_http_request.c`
- Modify: `firmware/tests/CMakeLists.txt` (the `foreach` line)

- [ ] **Step 1: Write the header and the failing test**

`firmware/src/http_request.h`:

```c
// Request encoders for talking to the printer over IPP-over-USB.
#ifndef HTTP_REQUEST_H
#define HTTP_REQUEST_H

#include <stddef.h>
#include <stdint.h>

// Writes an HTTP/1.1 request head the way ipp-usb sends it to USB printers. content_type may be
// NULL for a request without a body. Returns the head's length, or 0 if it does not fit.
size_t http_request_head(char *buf, size_t cap, const char *method, const char *path,
                         const char *content_type, size_t body_len);

// Writes an IPP 2.0 Get-Printer-Attributes request for every attribute. Returns its length, or
// 0 if it does not fit.
size_t ipp_get_printer_attributes(uint8_t *buf, size_t cap, uint32_t request_id);

#endif
```

`firmware/tests/test_http_request.c` (the golden bytes are `probe/httpipp.py`'s request, which the
printer answered):

```c
#include <string.h>

#include "check.h"
#include "http_request.h"

static void test_get_head(void) {
    char buf[256];
    size_t len = http_request_head(buf, sizeof buf, "GET", "/eSCL/ScannerCapabilities", NULL, 0);
    const char *want = "GET /eSCL/ScannerCapabilities HTTP/1.1\r\nHost: localhost\r\n"
                       "Connection: keep-alive\r\n\r\n";
    CHECK(len == strlen(want) && strcmp(buf, want) == 0);
}

static void test_post_head(void) {
    char buf[256];
    size_t len = http_request_head(buf, sizeof buf, "POST", "/ipp/print", "application/ipp", 141);
    const char *want = "POST /ipp/print HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n"
                       "Content-Type: application/ipp\r\nContent-Length: 141\r\n\r\n";
    CHECK(len == strlen(want) && strcmp(buf, want) == 0);
}

static void test_head_that_does_not_fit(void) {
    char buf[16];
    CHECK(http_request_head(buf, sizeof buf, "GET", "/eSCL/ScannerCapabilities", NULL, 0) == 0);
}

// The same bytes as probe/httpipp.py's encode_get_printer_attributes(request_id=1), which the
// printer answered over USB.
static const uint8_t GOLDEN_IPP[] = {
    0x02, 0x00, 0x00, 0x0b, 0x00, 0x00, 0x00, 0x01, 0x01, 0x47, 0x00, 0x12, 0x61, 0x74, 0x74,
    0x72, 0x69, 0x62, 0x75, 0x74, 0x65, 0x73, 0x2d, 0x63, 0x68, 0x61, 0x72, 0x73, 0x65, 0x74,
    0x00, 0x05, 0x75, 0x74, 0x66, 0x2d, 0x38, 0x48, 0x00, 0x1b, 0x61, 0x74, 0x74, 0x72, 0x69,
    0x62, 0x75, 0x74, 0x65, 0x73, 0x2d, 0x6e, 0x61, 0x74, 0x75, 0x72, 0x61, 0x6c, 0x2d, 0x6c,
    0x61, 0x6e, 0x67, 0x75, 0x61, 0x67, 0x65, 0x00, 0x02, 0x65, 0x6e, 0x45, 0x00, 0x0b, 0x70,
    0x72, 0x69, 0x6e, 0x74, 0x65, 0x72, 0x2d, 0x75, 0x72, 0x69, 0x00, 0x19, 0x69, 0x70, 0x70,
    0x3a, 0x2f, 0x2f, 0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x68, 0x6f, 0x73, 0x74, 0x2f, 0x69, 0x70,
    0x70, 0x2f, 0x70, 0x72, 0x69, 0x6e, 0x74, 0x44, 0x00, 0x14, 0x72, 0x65, 0x71, 0x75, 0x65,
    0x73, 0x74, 0x65, 0x64, 0x2d, 0x61, 0x74, 0x74, 0x72, 0x69, 0x62, 0x75, 0x74, 0x65, 0x73,
    0x00, 0x03, 0x61, 0x6c, 0x6c, 0x03,
};

static void test_get_printer_attributes_matches_the_probe(void) {
    uint8_t buf[256];
    size_t len = ipp_get_printer_attributes(buf, sizeof buf, 1);
    CHECK(len == sizeof GOLDEN_IPP);
    CHECK(memcmp(buf, GOLDEN_IPP, sizeof GOLDEN_IPP) == 0);
}

static void test_request_id_is_big_endian(void) {
    uint8_t buf[256];
    ipp_get_printer_attributes(buf, sizeof buf, 0x01020304);
    CHECK(buf[4] == 0x01 && buf[5] == 0x02 && buf[6] == 0x03 && buf[7] == 0x04);
}

static void test_ipp_that_does_not_fit(void) {
    uint8_t buf[100];
    CHECK(ipp_get_printer_attributes(buf, sizeof buf, 1) == 0);
}

int main(void) {
    test_get_head();
    test_post_head();
    test_head_that_does_not_fit();
    test_get_printer_attributes_matches_the_probe();
    test_request_id_is_big_endian();
    test_ipp_that_does_not_fit();
    return CHECK_RESULT();
}
```

In `firmware/tests/CMakeLists.txt` change the loop to:

```cmake
foreach(module usb_desc http_request)
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `just fw-test`
Expected: CMake error `Cannot find source file: .../firmware/src/http_request.c`.

- [ ] **Step 3: Implement**

`firmware/src/http_request.c`:

```c
#include "http_request.h"

#include <stdbool.h>
#include <stdio.h>
#include <string.h>

size_t http_request_head(char *buf, size_t cap, const char *method, const char *path,
                         const char *content_type, size_t body_len) {
    int written;
    if (content_type == NULL && body_len == 0) {
        written = snprintf(buf, cap,
                           "%s %s HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n",
                           method, path);
    } else {
        written = snprintf(buf, cap,
                           "%s %s HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n"
                           "Content-Type: %s\r\nContent-Length: %zu\r\n\r\n",
                           method, path, content_type != NULL ? content_type : "", body_len);
    }
    if (written < 0 || (size_t)written >= cap) {
        return 0;
    }
    return (size_t)written;
}

typedef struct {
    uint8_t *buf;
    size_t cap;
    size_t len;
    bool overflow;
} writer_t;

static void put(writer_t *w, const void *data, size_t len) {
    if (w->overflow || w->len + len > w->cap) {
        w->overflow = true;
        return;
    }
    memcpy(w->buf + w->len, data, len);
    w->len += len;
}

static void put_u8(writer_t *w, uint8_t value) {
    put(w, &value, 1);
}

static void put_u16(writer_t *w, uint16_t value) {
    uint8_t bytes[2] = {(uint8_t)(value >> 8), (uint8_t)value};
    put(w, bytes, sizeof bytes);
}

static void put_attribute(writer_t *w, uint8_t tag, const char *name, const char *value) {
    put_u8(w, tag);
    put_u16(w, (uint16_t)strlen(name));
    put(w, name, strlen(name));
    put_u16(w, (uint16_t)strlen(value));
    put(w, value, strlen(value));
}

size_t ipp_get_printer_attributes(uint8_t *buf, size_t cap, uint32_t request_id) {
    writer_t w = {.buf = buf, .cap = cap};
    const uint8_t header[] = {
        0x02, 0x00, 0x00, 0x0b,
        (uint8_t)(request_id >> 24), (uint8_t)(request_id >> 16),
        (uint8_t)(request_id >> 8), (uint8_t)request_id,
        0x01,
    };
    put(&w, header, sizeof header);
    put_attribute(&w, 0x47, "attributes-charset", "utf-8");
    put_attribute(&w, 0x48, "attributes-natural-language", "en");
    put_attribute(&w, 0x45, "printer-uri", "ipp://localhost/ipp/print");
    put_attribute(&w, 0x44, "requested-attributes", "all");
    put_u8(&w, 0x03);
    return w.overflow ? 0 : w.len;
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `just fw-test`
Expected: `100% tests passed, 0 tests failed out of 2`.

- [ ] **Step 5: Commit**

```bash
git add firmware/src/http_request.h firmware/src/http_request.c firmware/tests
git commit -m "Encode HTTP request heads and IPP Get-Printer-Attributes in C"
```

---

### Task 4: HTTP response framing

The printer's IN endpoint is a byte stream: the framer decides where one response ends
(`Content-Length`, chunked encoding, or a bodiless status) and hands body bytes on as they arrive,
so neither the firmware nor, later, the WiFi proxy ever buffers a whole response. Interim `1xx`
responses are skipped because CUPS sends `Expect: 100-continue` with print jobs.

**Files:**
- Create: `firmware/src/http_framer.h`, `firmware/src/http_framer.c`, `firmware/tests/test_http_framer.c`
- Modify: `firmware/tests/CMakeLists.txt` (the `foreach` line)

- [ ] **Step 1: Write the header and the failing test**

`firmware/src/http_framer.h`:

```c
// Finds where one HTTP/1.1 response ends in a byte stream with no message boundaries, such as
// an IPP-over-USB bulk IN endpoint. Body bytes are handed to a callback as they arrive.
#ifndef HTTP_FRAMER_H
#define HTTP_FRAMER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define HTTP_FRAMER_HEAD_MAX 2048
#define HTTP_FRAMER_LINE_MAX 64

typedef enum {
    HTTP_FRAMER_HEAD,
    HTTP_FRAMER_BODY,
    HTTP_FRAMER_CHUNK_SIZE,
    HTTP_FRAMER_CHUNK_DATA,
    HTTP_FRAMER_CHUNK_END,
    HTTP_FRAMER_TRAILER,
    HTTP_FRAMER_DONE,
    HTTP_FRAMER_ERROR,
} http_framer_state_t;

typedef void (*http_body_fn)(void *ctx, const uint8_t *data, size_t len);

typedef struct {
    http_framer_state_t state;
    int status;
    bool chunked;
    uint32_t content_length;
    uint32_t remaining;
    const char *error;
    http_body_fn on_body;
    void *ctx;
    char head[HTTP_FRAMER_HEAD_MAX + 1];
    size_t head_len;
    char line[HTTP_FRAMER_LINE_MAX + 1];
    size_t line_len;
    bool line_overflow;
} http_framer_t;

void http_framer_init(http_framer_t *framer, http_body_fn on_body, void *ctx);

// Consumes bytes of one response and returns how many it used. It uses fewer than len only once
// the response is complete (state DONE) or malformed (state ERROR, reason in framer->error).
// Interim 1xx responses are skipped; the response that follows them is framed.
size_t http_framer_feed(http_framer_t *framer, const uint8_t *data, size_t len);

#endif
```

`firmware/tests/test_http_framer.c`:

```c
#include <string.h>

#include "check.h"
#include "http_framer.h"

typedef struct {
    char body[256];
    size_t len;
} sink_t;

static void collect(void *ctx, const uint8_t *data, size_t len) {
    sink_t *sink = ctx;
    memcpy(sink->body + sink->len, data, len);
    sink->len += len;
}

// Feeds text in pieces of `step` bytes; returns the total consumed.
static size_t feed(http_framer_t *framer, const char *text, size_t step) {
    size_t total = strlen(text);
    size_t used = 0;
    for (size_t offset = 0; offset < total; offset += step) {
        size_t piece = total - offset < step ? total - offset : step;
        size_t consumed = http_framer_feed(framer, (const uint8_t *)text + offset, piece);
        used += consumed;
        if (consumed < piece) {
            break;
        }
    }
    return used;
}

static void test_content_length_in_any_split(void) {
    const char *response = "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\n"
                           "Content-Length: 5\r\n\r\nhello";
    for (size_t step = 1; step <= strlen(response); step++) {
        sink_t sink = {0};
        http_framer_t framer;
        http_framer_init(&framer, collect, &sink);
        CHECK(feed(&framer, response, step) == strlen(response));
        CHECK(framer.state == HTTP_FRAMER_DONE);
        CHECK(framer.status == 200);
        CHECK(framer.content_length == 5);
        CHECK(sink.len == 5 && memcmp(sink.body, "hello", 5) == 0);
    }
}

static void test_stops_at_the_end_of_the_response(void) {
    const char *stream = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhiHTTP/1.1";
    http_framer_t framer;
    http_framer_init(&framer, NULL, NULL);
    CHECK(feed(&framer, stream, 64) == strlen(stream) - strlen("HTTP/1.1"));
    CHECK(framer.state == HTTP_FRAMER_DONE);
}

static void test_chunked_with_extension_and_trailer(void) {
    const char *response = "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, Chunked\r\n\r\n"
                           "4;name=x\r\nWiki\r\n5\r\npedia\r\n0\r\nExpires: never\r\n\r\n";
    for (size_t step = 1; step <= 7; step++) {
        sink_t sink = {0};
        http_framer_t framer;
        http_framer_init(&framer, collect, &sink);
        CHECK(feed(&framer, response, step) == strlen(response));
        CHECK(framer.state == HTTP_FRAMER_DONE);
        CHECK(sink.len == 9 && memcmp(sink.body, "Wikipedia", 9) == 0);
    }
}

static void test_interim_response_is_skipped(void) {
    sink_t sink = {0};
    http_framer_t framer;
    http_framer_init(&framer, collect, &sink);
    const char *stream = "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    CHECK(feed(&framer, stream, 3) == strlen(stream));
    CHECK(framer.state == HTTP_FRAMER_DONE);
    CHECK(framer.status == 200);
    CHECK(sink.len == 2);
}

static void test_responses_without_a_body(void) {
    const char *responses[] = {
        "HTTP/1.1 204 No Content\r\n\r\n",
        "HTTP/1.1 304 Not Modified\r\n\r\n",
        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n",
    };
    for (size_t i = 0; i < sizeof responses / sizeof responses[0]; i++) {
        http_framer_t framer;
        http_framer_init(&framer, NULL, NULL);
        CHECK(feed(&framer, responses[i], 64) == strlen(responses[i]));
        CHECK(framer.state == HTTP_FRAMER_DONE);
    }
}

static void expect_error(const char *stream, const char *error) {
    http_framer_t framer;
    http_framer_init(&framer, NULL, NULL);
    feed(&framer, stream, 64);
    CHECK(framer.state == HTTP_FRAMER_ERROR);
    CHECK(framer.error != NULL && strcmp(framer.error, error) == 0);
    if (framer.error != NULL && strcmp(framer.error, error) != 0) {
        fprintf(stderr, "  got \"%s\", want \"%s\"\n", framer.error, error);
    }
}

static void test_malformed_responses(void) {
    expect_error("SOAP/1.0 200 OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.1 2x0 OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.1 200 OK\r\nno colon\r\n\r\n", "malformed header line");
    expect_error("HTTP/1.1 200 OK\r\nContent-Length: 12a\r\n\r\n", "malformed Content-Length");
    expect_error("HTTP/1.1 200 OK\r\nContent-Length: 99999999999\r\n\r\n",
                 "malformed Content-Length");
    expect_error("HTTP/1.1 200 OK\r\n\r\n",
                 "response has neither Content-Length nor chunked encoding");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
                 "malformed chunk size");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n123456789\r\n",
                 "chunk too large");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nabc\r\n",
                 "chunk data longer than its size");
}

static void test_head_too_large(void) {
    static char stream[HTTP_FRAMER_HEAD_MAX + 64];
    strcpy(stream, "HTTP/1.1 200 OK\r\nX-Filler: ");
    size_t len = strlen(stream);
    memset(stream + len, 'a', HTTP_FRAMER_HEAD_MAX);
    stream[len + HTTP_FRAMER_HEAD_MAX] = '\0';
    expect_error(stream, "response head too large");
}

int main(void) {
    test_content_length_in_any_split();
    test_stops_at_the_end_of_the_response();
    test_chunked_with_extension_and_trailer();
    test_interim_response_is_skipped();
    test_responses_without_a_body();
    test_malformed_responses();
    test_head_too_large();
    return CHECK_RESULT();
}
```

In `firmware/tests/CMakeLists.txt` change the loop to:

```cmake
foreach(module http_framer usb_desc http_request)
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `just fw-test`
Expected: CMake error `Cannot find source file: .../firmware/src/http_framer.c`.

- [ ] **Step 3: Implement**

`firmware/src/http_framer.c`:

```c
#include "http_framer.h"

#include <ctype.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>

static size_t fail(http_framer_t *framer, const char *error) {
    framer->state = HTTP_FRAMER_ERROR;
    framer->error = error;
    return 0;
}

void http_framer_init(http_framer_t *framer, http_body_fn on_body, void *ctx) {
    memset(framer, 0, sizeof *framer);
    framer->state = HTTP_FRAMER_HEAD;
    framer->on_body = on_body;
    framer->ctx = ctx;
}

static bool parse_decimal(const char *text, uint32_t *value) {
    while (*text == ' ' || *text == '\t') {
        text++;
    }
    if (!isdigit((unsigned char)*text)) {
        return false;
    }
    char *end = NULL;
    unsigned long long parsed = strtoull(text, &end, 10);
    while (*end == ' ' || *end == '\t') {
        end++;
    }
    if (*end != '\0' || parsed > UINT32_MAX) {
        return false;
    }
    *value = (uint32_t)parsed;
    return true;
}

static bool contains_token(const char *value, const char *token) {
    size_t token_len = strlen(token);
    for (const char *p = value; *p != '\0'; p++) {
        if (strncasecmp(p, token, token_len) == 0) {
            return true;
        }
    }
    return false;
}

// Reads the header fields after the status line; returns an error message or NULL.
static const char *parse_fields(http_framer_t *framer, char *fields, bool *has_length) {
    for (char *line = strtok(fields, "\r\n"); line != NULL; line = strtok(NULL, "\r\n")) {
        char *colon = strchr(line, ':');
        if (colon == NULL) {
            return "malformed header line";
        }
        *colon = '\0';
        const char *value = colon + 1;
        if (strcasecmp(line, "Content-Length") == 0) {
            if (!parse_decimal(value, &framer->content_length)) {
                return "malformed Content-Length";
            }
            *has_length = true;
        } else if (strcasecmp(line, "Transfer-Encoding") == 0) {
            framer->chunked = contains_token(value, "chunked");
        }
    }
    return NULL;
}

static void start_body(http_framer_t *framer, bool has_length) {
    int status = framer->status;
    if (status == 204 || status == 304) {
        framer->state = HTTP_FRAMER_DONE;
    } else if (framer->chunked) {
        framer->state = HTTP_FRAMER_CHUNK_SIZE;
    } else if (has_length) {
        framer->remaining = framer->content_length;
        framer->state = framer->remaining > 0 ? HTTP_FRAMER_BODY : HTTP_FRAMER_DONE;
    } else {
        fail(framer, "response has neither Content-Length nor chunked encoding");
    }
}

// Returns the status code of "HTTP/1.x NNN ...", or -1.
static int parse_status_line(const char *head) {
    if (strncmp(head, "HTTP/1.", 7) != 0 || head[8] != ' ') {
        return -1;
    }
    for (int i = 9; i < 12; i++) {
        if (!isdigit((unsigned char)head[i])) {
            return -1;
        }
    }
    return (head[9] - '0') * 100 + (head[10] - '0') * 10 + (head[11] - '0');
}

static void parse_head(http_framer_t *framer) {
    framer->status = parse_status_line(framer->head);
    if (framer->status < 0) {
        fail(framer, "not an HTTP/1.x status line");
        return;
    }
    char *fields = strstr(framer->head, "\r\n") + 2;
    bool has_length = false;
    framer->chunked = false;
    const char *error = parse_fields(framer, fields, &has_length);
    if (error != NULL) {
        fail(framer, error);
    } else if (framer->status < 200) {
        framer->head_len = 0;
    } else {
        start_body(framer, has_length);
    }
}

static size_t feed_head(http_framer_t *framer, const uint8_t *data, size_t len) {
    size_t used = 0;
    while (used < len) {
        if (framer->head_len == HTTP_FRAMER_HEAD_MAX) {
            return used + fail(framer, "response head too large");
        }
        framer->head[framer->head_len++] = (char)data[used++];
        if (framer->head_len >= 4 &&
            memcmp(framer->head + framer->head_len - 4, "\r\n\r\n", 4) == 0) {
            framer->head[framer->head_len] = '\0';
            parse_head(framer);
            return used;
        }
    }
    return used;
}

static size_t feed_data(http_framer_t *framer, const uint8_t *data, size_t len,
                        http_framer_state_t next) {
    size_t take = len < framer->remaining ? len : framer->remaining;
    if (framer->on_body != NULL && take > 0) {
        framer->on_body(framer->ctx, data, take);
    }
    framer->remaining -= (uint32_t)take;
    if (framer->remaining == 0) {
        framer->state = next;
    }
    return take;
}

// Collects one CRLF-terminated line into framer->line; returns the bytes used and sets
// *complete once the CRLF has been seen. Characters past HTTP_FRAMER_LINE_MAX are dropped.
static size_t feed_line(http_framer_t *framer, const uint8_t *data, size_t len, bool *complete) {
    size_t used = 0;
    *complete = false;
    while (used < len) {
        char c = (char)data[used++];
        if (c == '\n' && framer->line_len > 0 && framer->line[framer->line_len - 1] == '\r') {
            framer->line[--framer->line_len] = '\0';
            *complete = true;
            return used;
        }
        if (framer->line_len < HTTP_FRAMER_LINE_MAX) {
            framer->line[framer->line_len++] = c;
        } else {
            framer->line_overflow = true;
            framer->line[framer->line_len - 1] = c;
        }
    }
    return used;
}

static void reset_line(http_framer_t *framer) {
    framer->line_len = 0;
    framer->line_overflow = false;
}

static void parse_chunk_size(http_framer_t *framer) {
    char *end = NULL;
    unsigned long long size = strtoull(framer->line, &end, 16);
    bool has_digits = end != framer->line;
    while (*end == ' ' || *end == '\t') {
        end++;
    }
    if (framer->line_overflow || !has_digits || (*end != '\0' && *end != ';')) {
        fail(framer, "malformed chunk size");
    } else if (size > UINT32_MAX) {
        fail(framer, "chunk too large");
    } else if (size == 0) {
        framer->state = HTTP_FRAMER_TRAILER;
    } else {
        framer->remaining = (uint32_t)size;
        framer->state = HTTP_FRAMER_CHUNK_DATA;
    }
    reset_line(framer);
}

static size_t feed_chunk_line(http_framer_t *framer, const uint8_t *data, size_t len) {
    bool complete = false;
    size_t used = feed_line(framer, data, len, &complete);
    if (!complete) {
        return used;
    }
    if (framer->state == HTTP_FRAMER_CHUNK_SIZE) {
        parse_chunk_size(framer);
    } else if (framer->state == HTTP_FRAMER_CHUNK_END) {
        if (framer->line_len != 0) {
            return used + fail(framer, "chunk data longer than its size");
        }
        framer->state = HTTP_FRAMER_CHUNK_SIZE;
    } else if (framer->line_len == 0 && !framer->line_overflow) {
        framer->state = HTTP_FRAMER_DONE;
    }
    reset_line(framer);
    return used;
}

static size_t feed_body(http_framer_t *framer, const uint8_t *data, size_t len) {
    return feed_data(framer, data, len, HTTP_FRAMER_DONE);
}

static size_t feed_chunk_data(http_framer_t *framer, const uint8_t *data, size_t len) {
    return feed_data(framer, data, len, HTTP_FRAMER_CHUNK_END);
}

typedef size_t (*feed_fn)(http_framer_t *framer, const uint8_t *data, size_t len);

static const feed_fn FEEDERS[] = {
    [HTTP_FRAMER_HEAD] = feed_head,
    [HTTP_FRAMER_BODY] = feed_body,
    [HTTP_FRAMER_CHUNK_SIZE] = feed_chunk_line,
    [HTTP_FRAMER_CHUNK_DATA] = feed_chunk_data,
    [HTTP_FRAMER_CHUNK_END] = feed_chunk_line,
    [HTTP_FRAMER_TRAILER] = feed_chunk_line,
};

size_t http_framer_feed(http_framer_t *framer, const uint8_t *data, size_t len) {
    size_t used = 0;
    while (used < len && framer->state != HTTP_FRAMER_DONE && framer->state != HTTP_FRAMER_ERROR) {
        used += FEEDERS[framer->state](framer, data + used, len - used);
    }
    return used;
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `just fw-test`
Expected: `100% tests passed, 0 tests failed out of 3`.

- [ ] **Step 5: Check the tests catch real bugs**

Break the code three ways, one at a time, run `just fw-test` after each, and restore the line
afterwards (`git checkout firmware/src/http_framer.c` once it is committed, or undo the edit):

1. In `parse_head`, replace `framer->head_len = 0;` with `;` (interim responses no longer skipped).
2. In `start_body`, replace `status == 204 || status == 304` with `status == 204`.
3. In `feed_chunk_line`, replace `if (framer->line_len != 0) {` with `if (framer->line_len == 99) {`.

Expected: each one makes `http_framer` fail.

- [ ] **Step 6: Commit**

```bash
git add firmware/src/http_framer.h firmware/src/http_framer.c firmware/tests
git commit -m "Frame HTTP/1.1 responses from a byte stream"
```

---

### Task 5: Firmware build

**Files:**
- Create: `firmware/CMakeLists.txt`, `firmware/src/tusb_config.h`
- Create: `firmware/src/printer_link.h`, `firmware/src/printer_link.c`, `firmware/src/main.c`
- Modify: `justfile`

How `printer_link.c` works, since TinyUSB's bulk transfers are asynchronous only:

- `tuh_mount_cb` (runs inside `tuh_task`) records an HP device; `printer_link_task` (main loop)
  then reads the configuration descriptor with the blocking helper, which is not allowed inside
  callbacks, finds `ff/09/01` with `usb_find_bulk_pair` and opens both endpoints.
- An exchange writes the request on OUT (plus a zero-length packet when its length is a multiple
  of the packet size), then keeps one read queued on IN. Data goes through the framer. A
  zero-length read means "nothing yet": the next read is queued 1 ms later from the main loop
  instead of immediately, so the idle printer's stream of ZLPs does not monopolise the bus.
- Silence is measured by the clock: 5 s without data aborts the read and fails the exchange.

- [ ] **Step 1: Add the build recipe to `justfile`**

```make
# Cross-compile the firmware: build/fw/m130a_bridge.{elf,uf2}.
fw-build:
    cmake -S firmware -B build/fw -G Ninja
    cmake --build build/fw
    arm-none-eabi-size build/fw/m130a_bridge.elf
```

- [ ] **Step 2: Write the build files**

`firmware/CMakeLists.txt`:

```cmake
# Firmware for the Pico 2 W. PICO_SDK_PATH and PICO_TINYUSB_PATH come from the Nix dev shell.
cmake_minimum_required(VERSION 3.20)

set(PICO_BOARD pico2_w)
set(PICO_PLATFORM rp2350-arm-s)
include($ENV{PICO_SDK_PATH}/pico_sdk_init.cmake)

project(m130a_bridge C CXX ASM)
set(CMAKE_C_STANDARD 11)
pico_sdk_init()

add_executable(m130a_bridge
    src/main.c
    src/printer_link.c
    src/http_framer.c
    src/http_request.c
    src/usb_desc.c
)
target_include_directories(m130a_bridge PRIVATE src)
target_link_libraries(m130a_bridge PRIVATE pico_stdlib tinyusb_host)
set_source_files_properties(
    src/main.c src/printer_link.c src/http_framer.c src/http_request.c src/usb_desc.c
    PROPERTIES COMPILE_OPTIONS "-Wall;-Wextra;-Werror"
)

# Logs go to UART0 on GP0/GP1 (the Debug Probe); the USB port is the printer's host port.
pico_enable_stdio_uart(m130a_bridge 1)
pico_enable_stdio_usb(m130a_bridge 0)
pico_add_extra_outputs(m130a_bridge)
```

`firmware/src/tusb_config.h`:

```c
// TinyUSB configuration: host mode on the RP2350's native USB port, raw endpoint transfers only.
#ifndef TUSB_CONFIG_H
#define TUSB_CONFIG_H

#ifndef CFG_TUSB_OS
#define CFG_TUSB_OS OPT_OS_PICO
#endif

#define BOARD_TUH_RHPORT 0
#define CFG_TUH_ENABLED 1
#define CFG_TUH_MAX_SPEED OPT_MODE_FULL_SPEED

// The printer's configuration descriptor is 168 bytes.
#define CFG_TUH_ENUMERATION_BUFSIZE 256
#define CFG_TUH_HUB 0
#define CFG_TUH_DEVICE_MAX 1
#define CFG_TUH_ENDPOINT_MAX 8
#define CFG_TUH_API_EDPT_XFER 1

#endif
```

- [ ] **Step 3: Write the USB link**

`firmware/src/printer_link.h`:

```c
// HTTP exchanges with the printer over its IPP-over-USB interface, driven from the main loop.
#ifndef PRINTER_LINK_H
#define PRINTER_LINK_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef void (*printer_body_fn)(void *ctx, const uint8_t *data, size_t len);

typedef struct {
    const char *error; // NULL when the exchange succeeded
    int http_status;
    uint32_t body_bytes;
    uint32_t elapsed_ms;
} printer_result_t;

// Call from the main loop after tuh_task(): sets the printer up once it is mounted, re-arms
// reads and enforces the response timeout.
void printer_link_task(void);

// True once an HP printer is mounted and its IPP-over-USB endpoints are open.
bool printer_link_ready(void);

// Starts one exchange: writes `request` (head and body; it must stay valid until the exchange
// ends and be at most 65535 bytes) and streams the response body to on_body. Returns false if
// the printer is not ready or an exchange is already running.
bool printer_link_start(const uint8_t *request, size_t len, printer_body_fn on_body, void *ctx);

bool printer_link_busy(void);

// The outcome of the last finished exchange.
printer_result_t printer_link_result(void);

#endif
```

`firmware/src/printer_link.c`:

```c
#include "printer_link.h"

#include <stdio.h>
#include <string.h>

#include "http_framer.h"
#include "pico/time.h"
#include "tusb.h"
#include "usb_desc.h"

#define HP_VENDOR_ID 0x03F0
#define IPP_CLASS 0xFF
#define IPP_SUBCLASS 0x09
#define IPP_PROTOCOL 0x01
#define CONFIG_BUF_SIZE 512
#define IN_BUF_SIZE 512
// Idle IPP endpoints answer every read at once with a zero-length packet; wait this long
// before asking again instead of spinning.
#define IDLE_POLL_US 1000
#define QUIET_TIMEOUT_US (5 * 1000 * 1000)

static struct {
    uint8_t daddr;
    bool setup_pending;
    bool ready;
    usb_bulk_pair_t ipp;
    uint8_t config[CONFIG_BUF_SIZE];
} printer;

static struct {
    bool busy;
    bool zlp_pending;
    bool in_armed;
    const uint8_t *request;
    size_t request_len;
    http_framer_t framer;
    printer_body_fn on_body;
    void *ctx;
    uint64_t started_us;
    uint64_t last_data_us;
    uint64_t rearm_at_us;
    printer_result_t result;
} exchange;

static uint8_t in_buf[IN_BUF_SIZE] __attribute__((aligned(4)));
static uint8_t zlp_buf[4] __attribute__((aligned(4)));

static void finish(const char *error) {
    exchange.busy = false;
    exchange.result.error = error;
    exchange.result.http_status = exchange.framer.status;
    exchange.result.elapsed_ms = (uint32_t)((time_us_64() - exchange.started_us) / 1000);
}

static void count_body(void *ctx, const uint8_t *data, size_t len) {
    (void)ctx;
    exchange.result.body_bytes += (uint32_t)len;
    if (exchange.on_body != NULL) {
        exchange.on_body(exchange.ctx, data, len);
    }
}

static void in_complete(tuh_xfer_t *xfer);

static void arm_in(void) {
    exchange.rearm_at_us = 0;
    tuh_xfer_t xfer = {
        .daddr = printer.daddr,
        .ep_addr = printer.ipp.ep_in,
        .buflen = sizeof in_buf,
        .buffer = in_buf,
        .complete_cb = in_complete,
        .user_data = 0,
    };
    exchange.in_armed = tuh_edpt_xfer(&xfer);
    if (!exchange.in_armed) {
        finish("could not queue a read on the IPP IN endpoint");
    }
}

static void in_complete(tuh_xfer_t *xfer) {
    exchange.in_armed = false;
    if (!exchange.busy) {
        return;
    }
    if (xfer->result != XFER_RESULT_SUCCESS) {
        finish("reading the response failed");
        return;
    }
    if (xfer->actual_len == 0) {
        exchange.rearm_at_us = time_us_64() + IDLE_POLL_US;
        return;
    }
    exchange.last_data_us = time_us_64();
    size_t used = http_framer_feed(&exchange.framer, in_buf, xfer->actual_len);
    if (exchange.framer.state == HTTP_FRAMER_ERROR) {
        finish(exchange.framer.error);
    } else if (exchange.framer.state == HTTP_FRAMER_DONE) {
        if (used < xfer->actual_len) {
            printf("printer: %lu bytes after the response were dropped\n",
                   (unsigned long)(xfer->actual_len - used));
        }
        finish(NULL);
    } else {
        arm_in();
    }
}

static void out_complete(tuh_xfer_t *xfer);

static bool submit_out(uint8_t *data, size_t len) {
    tuh_xfer_t xfer = {
        .daddr = printer.daddr,
        .ep_addr = printer.ipp.ep_out,
        .buflen = (uint32_t)len,
        .buffer = data,
        .complete_cb = out_complete,
        .user_data = 0,
    };
    return tuh_edpt_xfer(&xfer);
}

static void out_complete(tuh_xfer_t *xfer) {
    if (!exchange.busy) {
        return;
    }
    if (xfer->result != XFER_RESULT_SUCCESS) {
        finish("writing the request failed");
        return;
    }
    if (exchange.zlp_pending) {
        exchange.zlp_pending = false;
        if (!submit_out(zlp_buf, 0)) {
            finish("could not queue the zero-length packet");
        }
        return;
    }
    exchange.last_data_us = time_us_64();
    arm_in();
}

static void set_up_printer(void) {
    printer.setup_pending = false;
    tusb_xfer_result_t result = tuh_descriptor_get_configuration_sync(
        printer.daddr, 0, printer.config, sizeof printer.config);
    if (result != XFER_RESULT_SUCCESS) {
        printf("printer: reading the configuration descriptor failed (%d)\n", result);
        return;
    }
    size_t total = (size_t)(printer.config[2] | printer.config[3] << 8);
    if (total > sizeof printer.config) {
        printf("printer: configuration descriptor is %u bytes, more than %u\n",
               (unsigned)total, (unsigned)sizeof printer.config);
        return;
    }
    if (!usb_find_bulk_pair(printer.config, total, IPP_CLASS, IPP_SUBCLASS, IPP_PROTOCOL,
                            &printer.ipp)) {
        printf("printer: no IPP-over-USB (ff/09/01) interface\n");
        return;
    }
    const tusb_desc_endpoint_t *out = (const void *)&printer.config[printer.ipp.ep_out_offset];
    const tusb_desc_endpoint_t *in = (const void *)&printer.config[printer.ipp.ep_in_offset];
    if (!tuh_edpt_open(printer.daddr, out) || !tuh_edpt_open(printer.daddr, in)) {
        printf("printer: opening the IPP endpoints failed\n");
        return;
    }
    printer.ready = true;
    printf("printer: IPP on interface %u, OUT 0x%02x / IN 0x%02x, %u-byte packets, %s speed\n",
           printer.ipp.interface_number, printer.ipp.ep_out, printer.ipp.ep_in,
           printer.ipp.ep_in_size,
           tuh_speed_get(printer.daddr) == TUSB_SPEED_HIGH ? "high" : "full");
}

void tuh_mount_cb(uint8_t daddr) {
    uint16_t vid = 0;
    uint16_t pid = 0;
    tuh_vid_pid_get(daddr, &vid, &pid);
    printf("usb: device %04x:%04x mounted at address %u\n", vid, pid, daddr);
    if (vid == HP_VENDOR_ID && printer.daddr == 0) {
        printer.daddr = daddr;
        printer.setup_pending = true;
    }
}

void tuh_umount_cb(uint8_t daddr) {
    printf("usb: device at address %u removed\n", daddr);
    if (daddr != printer.daddr) {
        return;
    }
    memset(&printer, 0, sizeof printer);
    if (exchange.busy) {
        finish("printer disconnected");
    }
}

void printer_link_task(void) {
    if (printer.setup_pending) {
        set_up_printer();
    }
    if (!exchange.busy) {
        return;
    }
    uint64_t now = time_us_64();
    if (!exchange.in_armed && exchange.rearm_at_us != 0 && now >= exchange.rearm_at_us) {
        arm_in();
    }
    if (exchange.busy && now - exchange.last_data_us > QUIET_TIMEOUT_US) {
        tuh_edpt_abort_xfer(printer.daddr, printer.ipp.ep_in);
        finish("the printer sent nothing for 5 s");
    }
}

bool printer_link_ready(void) {
    return printer.ready;
}

bool printer_link_busy(void) {
    return exchange.busy;
}

bool printer_link_start(const uint8_t *request, size_t len, printer_body_fn on_body, void *ctx) {
    if (!printer.ready || exchange.busy || len == 0 || len > UINT16_MAX) {
        return false;
    }
    memset(&exchange, 0, sizeof exchange);
    exchange.busy = true;
    exchange.request = request;
    exchange.request_len = len;
    exchange.on_body = on_body;
    exchange.ctx = ctx;
    exchange.zlp_pending = len % printer.ipp.ep_out_size == 0;
    exchange.started_us = time_us_64();
    exchange.last_data_us = exchange.started_us;
    http_framer_init(&exchange.framer, count_body, NULL);
    if (!submit_out((uint8_t *)request, len)) {
        finish("could not queue the request write");
        return false;
    }
    return true;
}

printer_result_t printer_link_result(void) {
    return exchange.result;
}
```

- [ ] **Step 4: Write the milestone-1 app**

`firmware/src/main.c`:

```c
// Milestone 1: talk to the printer over USB from the Pico. Once the printer is mounted, ask it
// for its IPP attributes and its eSCL scanner capabilities and log the outcome on UART0.
#include <stdio.h>
#include <string.h>

#include "http_request.h"
#include "pico/stdlib.h"
#include "printer_link.h"
#include "tusb.h"

#define REQUEST_MAX 512
#define PREVIEW_MAX 160

typedef struct {
    const char *name;
    size_t (*build)(uint8_t *buf, size_t cap);
} probe_t;

static uint8_t request[REQUEST_MAX];
static char preview[PREVIEW_MAX + 1];
static size_t preview_len;

static size_t build_ipp(uint8_t *buf, size_t cap) {
    uint8_t body[256];
    size_t body_len = ipp_get_printer_attributes(body, sizeof body, 1);
    size_t head_len = http_request_head((char *)buf, cap, "POST", "/ipp/print",
                                        "application/ipp", body_len);
    if (body_len == 0 || head_len == 0 || head_len + body_len > cap) {
        return 0;
    }
    memcpy(buf + head_len, body, body_len);
    return head_len + body_len;
}

static size_t build_escl(uint8_t *buf, size_t cap) {
    return http_request_head((char *)buf, cap, "GET", "/eSCL/ScannerCapabilities", NULL, 0);
}

static const probe_t PROBES[] = {
    {"IPP Get-Printer-Attributes", build_ipp},
    {"eSCL ScannerCapabilities", build_escl},
};
#define PROBE_COUNT (sizeof PROBES / sizeof PROBES[0])

// Keeps the printable start of the body, to show the model name from either answer.
static void keep_preview(void *ctx, const uint8_t *data, size_t len) {
    (void)ctx;
    for (size_t i = 0; i < len && preview_len < PREVIEW_MAX; i++) {
        char c = (char)data[i];
        preview[preview_len++] = (c >= 0x20 && c < 0x7F) ? c : '.';
    }
    preview[preview_len] = '\0';
}

static void report(const probe_t *probe) {
    printer_result_t result = printer_link_result();
    if (result.error != NULL) {
        printf("%s: FAILED after %lu ms: %s\n", probe->name, (unsigned long)result.elapsed_ms,
               result.error);
        return;
    }
    printf("%s: HTTP %d, %lu body bytes in %lu ms\n  %s\n", probe->name, result.http_status,
           (unsigned long)result.body_bytes, (unsigned long)result.elapsed_ms, preview);
}

// Runs the probes one after the other, once per mount.
static void probe_task(void) {
    static size_t next;
    static bool running;
    if (!printer_link_ready()) {
        next = 0;
        running = false;
        return;
    }
    if (running && !printer_link_busy()) {
        report(&PROBES[next]);
        running = false;
        next++;
    }
    if (running || next >= PROBE_COUNT) {
        return;
    }
    size_t len = PROBES[next].build(request, sizeof request);
    preview_len = 0;
    preview[0] = '\0';
    running = len > 0 && printer_link_start(request, len, keep_preview, NULL);
    if (!running) {
        printf("%s: could not start\n", PROBES[next].name);
        next++;
    }
}

int main(void) {
    stdio_init_all();
    printf("\nm130a-bridge: USB host bring-up\n");
    tusb_rhport_init_t host_init = {.role = TUSB_ROLE_HOST, .speed = TUSB_SPEED_AUTO};
    tusb_init(BOARD_TUH_RHPORT, &host_init);
    while (true) {
        tuh_task();
        printer_link_task();
        probe_task();
    }
}
```

- [ ] **Step 5: Build**

Run: `just fw-build`
Expected: configure prints `Using PICO_TINYUSB_PATH from environment (...)`, `TinyUSB available
at .../hw/bsp/rp2040; enabling build support for USB.` and `Using picotool from
/nix/store/...-picotool-2.3.1/bin/picotool`; the build has no `warning:` lines; the size line reads
about `text 45608, data 0, bss 6164`. If configure says it is building picotool from source,
`picotool_DIR` is not set: re-enter the dev shell.

- [ ] **Step 6: Run the host tests again**

Run: `just fw-test`
Expected: `100% tests passed, 0 tests failed out of 3`.

- [ ] **Step 7: Commit**

```bash
git add justfile firmware/CMakeLists.txt firmware/src
git commit -m "Add Pico firmware that probes the printer over USB"
```

---

### Task 6: Flash and log recipes

**Files:**
- Modify: `justfile`

- [ ] **Step 1: Add the recipes**

```make
# Flash through the Debug Probe's SWD port and restart the Pico.
flash: fw-build
    probe-rs download --chip RP235x build/fw/m130a_bridge.elf
    probe-rs reset --chip RP235x

# The Pico's UART0 log, through the Debug Probe. Quit with ctrl-t q.
logs:
    tio --baudrate 115200 "$(ls /dev/cu.usbmodem* | head -n 1)"
```

- [ ] **Step 2: Check the recipes parse**

Run: `just --list`
Expected: `flash`, `fw-build`, `fw-test`, `lint`, `logs`, `probe`, `test` are listed.

- [ ] **Step 3: Commit**

```bash
git add justfile
git commit -m "Add flash and UART log recipes"
```

---

### Task 7: Wire up and run on the hardware

Follow [docs/wiring.svg](../../wiring.svg). Summary:

| From | To |
| ---- | -- |
| 5 V supply + (red) | Pico pin 40 (VBUS) |
| 5 V supply − (black) | Pico pin 38 (GND) |
| Debug Probe "U" orange (probe TX) | Pico pin 2 (GP1, UART0 RX) |
| Debug Probe "U" yellow (probe RX) | Pico pin 1 (GP0, UART0 TX) |
| Debug Probe "U" black | Pico pin 3 (GND) |
| Debug Probe "D" orange / black / yellow | Pico debug pads SWCLK / GND / SWDIO |
| Pico micro-USB | OTG adapter → USB-A to USB-B cable → printer |
| Debug Probe USB | Mac |

The Pico's micro-USB is the host port: never connect it to the Mac. The 5 V on pin 40 powers the
Pico and reaches the printer through the OTG adapter as VBUS, which the printer needs to attach.

- [ ] **Step 1: Power up without the printer and flash**

Connect everything except the printer cable. Power the 5 V supply. In one terminal: `just logs`.
In another: `just flash`.
Expected in the log: `m130a-bridge: USB host bring-up`. If nothing appears, swap the orange and
yellow UART wires. If `probe-rs` cannot find the chip, check the SWD wiring and that the 5 V supply
is on (the probe does not power the Pico).

- [ ] **Step 2: Plug the printer in (printer switched on)**

Expected log, in this order (timings and the preview text will differ):

```
usb: device 03f0:622a mounted at address 1
printer: IPP on interface 3, OUT 0x0b / IN 0x8b, 64-byte packets, full speed
IPP Get-Printer-Attributes: HTTP 200, 9503 body bytes in ... ms
  ......G.attributes-charset..utf-8H..attributes-natural-language..en...
eSCL ScannerCapabilities: HTTP 200, 2875 body bytes in ... ms
  <?xml version="1.0" encoding="UTF-8"?><!-- THIS DATA SUBJECT TO DISCLAIMER(S) ...
```

`9503` and `2875` are the sizes the Mac probe got; small differences are fine (the attributes
include counters).

Troubleshooting:

| Log | Meaning |
| --- | ------- |
| nothing after the banner | the printer never attached: no 5 V on pin 40, or the adapter/cable chain is loose (the RP2350 sets host mode in software and ignores the adapter's ID pin) |
| `mounted` but `no IPP-over-USB (ff/09/01) interface` | the full-speed descriptor differs from the fixture: dump `printer.config` and compare with `firmware/tests/fixtures/` |
| `the printer sent nothing for 5 s` | the request reached the printer but no answer came: try IF4 (`ep_out 0x0c`) by skipping the first match, and check the request bytes against `probe/httpipp.py` |
| `writing the request failed` / `reading the response failed` | a USB-level error: note the exact line for the TinyUSB issue tracker (#3874 covers RP2350 host RX timeouts) |

- [ ] **Step 3: Unplug and replug the printer**

Expected: `usb: device at address 1 removed`, then the full sequence from Step 2 again.

---

### Task 8: Record the results

**Files:**
- Modify: `docs/research.md`

- [ ] **Step 1: Write down what the hardware did**

Under "## The printer", replace the line ending `Not yet tried.` in the **Speed** item with what
the log showed: the packet size and speed `printer_link` printed, and the two exchange timings.
Add any surprise from the troubleshooting table under the item it concerns.

- [ ] **Step 2: Commit**

```bash
git add docs/research.md
git commit -m "Record the Pico's first USB exchange with the printer"
```

---

## After milestone 1 (separate plans, written once this one has run on hardware)

1. **WiFi proxy.** CYW43 + lwIP (`pico_cyw43_arch_lwip_poll`, so lwIP is driven from the same main
   loop), a TCP listener on port 631 forwarding one HTTP exchange at a time to `printer_link`.
   Needs: framing for requests as well as responses (extend `http_framer` with a request mode),
   streaming request bodies from TCP to USB with `tcp_recved()` flow control, WiFi credentials from
   a gitignored header. Test: `ipptool -tv ipp://<pico-ip>:631/ipp/print get-printer-attributes.test`
   and `curl http://<pico-ip>:631/eSCL/ScannerCapabilities` from the Mac.
2. **Bonjour.** lwIP's mDNS responder advertising `_ipp._tcp` (subtype `_universal`, TXT from the
   printer's own attributes: `URF`, `pdl`, `UUID`, `rp=ipp/print`) and `_uscan._tcp` (`rs=eSCL`).
   Test: the printer and scanner appear in macOS's Printers & Scanners; print a page and scan one.
3. **Robustness.** Both IPP interfaces (IF3 and IF4) for two concurrent exchanges, watchdog,
   printer replug and WiFi reconnect handling, draining a response the client abandoned.
