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
    const http_head_t head = {
        .method = "POST",
        .path = "/ipp/print",
        .content_type = "application/ipp",
        .body_len = body_len,
    };
    size_t head_len = http_request_head((char *)buf, cap, &head);
    if (body_len == 0 || head_len == 0 || head_len + body_len > cap) {
        return 0;
    }
    memcpy(buf + head_len, body, body_len);
    return head_len + body_len;
}

static size_t build_escl(uint8_t *buf, size_t cap) {
    const http_head_t head = {
        .method = "GET",
        .path = "/eSCL/ScannerCapabilities",
        .content_type = NULL,
        .body_len = 0,
    };
    return http_request_head((char *)buf, cap, &head);
}

static const probe_t PROBES[] = {
    {.name = "IPP Get-Printer-Attributes", .build = build_ipp},
    {.name = "eSCL ScannerCapabilities", .build = build_escl},
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

static struct {
    size_t next;
    const probe_t *running; // NULL when no probe is in flight
} probes;

static void report_finished_probe(void) {
    if (probes.running == NULL || printer_link_busy()) {
        return;
    }
    report(probes.running);
    probes.running = NULL;
}

static void start_next_probe(void) {
    if (probes.running != NULL || probes.next >= PROBE_COUNT) {
        return;
    }
    const probe_t *probe = &PROBES[probes.next];
    probes.next++;
    size_t len = probe->build(request, sizeof request);
    if (len == 0) {
        printf("%s: the request does not fit in %u bytes\n", probe->name, REQUEST_MAX);
        return;
    }
    preview_len = 0;
    preview[0] = '\0';
    if (printer_link_start(request, len, keep_preview, NULL)) {
        probes.running = probe;
    } else {
        printf("%s: could not start: %s\n", probe->name, printer_link_result().error);
    }
}

// Runs the probes one after the other, once per mount. A probe cut short by an unplug is
// reported before the sequence resets.
static void probe_task(void) {
    report_finished_probe();
    if (!printer_link_ready()) {
        probes.next = 0;
        return;
    }
    start_next_probe();
}

int main(void) {
    stdio_init_all();
    printf("\nm130a-bridge: USB host bring-up\n");
    tusb_rhport_init_t host_init = {.role = TUSB_ROLE_HOST, .speed = TUSB_SPEED_AUTO};
    if (!tusb_init(BOARD_TUH_RHPORT, &host_init)) {
        printf("usb: host stack init failed\n");
        while (true) {
            tight_loop_contents();
        }
    }
    while (true) {
        tuh_task();
        printer_link_task();
        probe_task();
    }
}
