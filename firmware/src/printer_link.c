#include "printer_link.h"

#include <stdio.h>
#include <string.h>

#include "http_framer.h"
#include "pico/time.h"
#include "tusb.h"
#include "usb_desc.h"

#define HP_VENDOR_ID 0x03F0
#define CONFIG_BUF_SIZE 512
// Reads ask for one packet at a time: a multi-packet read arms both DPRAM buffers, and in host
// mode TinyUSB drops a packet that arrives after a short one, which the idle printer sends
// constantly. 64 bytes is the full-speed bulk packet size.
#define FS_BULK_PACKET_MAX 64
// Idle IPP endpoints answer every read at once with a zero-length packet; wait this long
// before asking again instead of spinning.
#define IDLE_POLL_US 1000
#define QUIET_TIMEOUT_US (5 * 1000 * 1000)

static const usb_interface_class_t IPP_OVER_USB = {.cls = 0xFF, .subclass = 0x09, .protocol = 0x01};

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
    http_framer_t framer;
    printer_body_fn on_body;
    void *ctx;
    uint32_t body_bytes;
    uint64_t started_us;
    uint64_t last_data_us;
    uint64_t rearm_at_us;
    printer_result_t result;
} exchange;

// Whether a transfer is queued on each endpoint. These outlive the exchange that queued them:
// the RP2350 host controller cannot abort a transfer, so a timed-out read stays queued until it
// completes or the printer is unplugged, and no new exchange may start before then.
static bool in_armed;
static bool out_armed;

static uint8_t in_buf[FS_BULK_PACKET_MAX] __attribute__((aligned(4)));
static uint8_t zlp_buf[4] __attribute__((aligned(4)));

static void finish(const char *error) {
    exchange.busy = false;
    exchange.result = (printer_result_t){
        .error = error,
        .http_status = exchange.framer.status,
        .body_bytes = exchange.body_bytes,
        .elapsed_ms = (uint32_t)((time_us_64() - exchange.started_us) / 1000),
    };
}

static void count_body(void *ctx, const uint8_t *data, size_t len) {
    (void)ctx;
    exchange.body_bytes += (uint32_t)len;
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
        .buflen = printer.ipp.ep_in_size,
        .buffer = in_buf,
        .complete_cb = in_complete,
        .user_data = 0,
    };
    in_armed = true;
    if (!tuh_edpt_xfer(&xfer)) {
        in_armed = false;
        finish("could not queue a read on the IPP IN endpoint");
    }
}

static void in_complete(tuh_xfer_t *xfer) {
    in_armed = false;
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
    out_armed = true;
    if (!tuh_edpt_xfer(&xfer)) {
        out_armed = false;
        return false;
    }
    return true;
}

static void out_complete(tuh_xfer_t *xfer) {
    out_armed = false;
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

// Reads the configuration descriptor into printer.config. Returns its length, or 0 on failure.
static size_t read_config(void) {
    tusb_xfer_result_t result = tuh_descriptor_get_configuration_sync(
        printer.daddr, 0, printer.config, sizeof printer.config);
    if (result != XFER_RESULT_SUCCESS) {
        printf("printer: reading the configuration descriptor failed (%d)\n", result);
        return 0;
    }
    size_t total = (size_t)(printer.config[2] | printer.config[3] << 8);
    if (total > sizeof printer.config) {
        printf("printer: configuration descriptor is %u bytes, more than %u\n",
               (unsigned)total, (unsigned)sizeof printer.config);
        return 0;
    }
    return total;
}

static bool ipp_endpoints_supported(const usb_bulk_pair_t *ipp) {
    if (tu_edpt_number(ipp->ep_out) >= CFG_TUH_ENDPOINT_MAX ||
        tu_edpt_number(ipp->ep_in) >= CFG_TUH_ENDPOINT_MAX) {
        printf("printer: IPP endpoints 0x%02x / 0x%02x exceed CFG_TUH_ENDPOINT_MAX (%d)\n",
               ipp->ep_out, ipp->ep_in, CFG_TUH_ENDPOINT_MAX);
        return false;
    }
    // A bigger descriptor value would be clamped to 64 by usbh at full speed, while the
    // zero-length-packet decision would still use the descriptor's value.
    if (ipp->ep_out_size == 0 || ipp->ep_out_size > FS_BULK_PACKET_MAX || ipp->ep_in_size == 0 ||
        ipp->ep_in_size > FS_BULK_PACKET_MAX) {
        printf("printer: IPP packet sizes OUT %u / IN %u unsupported (each must be 1-%d)\n",
               ipp->ep_out_size, ipp->ep_in_size, FS_BULK_PACKET_MAX);
        return false;
    }
    return true;
}

static void set_up_printer(void) {
    printer.setup_pending = false;
    size_t total = read_config();
    if (total == 0) {
        return;
    }
    if (!usb_find_bulk_pair(printer.config, total, IPP_OVER_USB, &printer.ipp)) {
        printf("printer: no IPP-over-USB (ff/09/01) interface\n");
        return;
    }
    if (!ipp_endpoints_supported(&printer.ipp)) {
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
    // Closing the device drops its queued transfers; their callbacks never run.
    in_armed = false;
    out_armed = false;
    if (exchange.busy) {
        finish("printer disconnected");
    }
}

static void rearm_if_due(uint64_t now) {
    if (exchange.busy && !in_armed && exchange.rearm_at_us != 0 && now >= exchange.rearm_at_us) {
        arm_in();
    }
}

static void enforce_quiet_timeout(uint64_t now) {
    if (!exchange.busy || now - exchange.last_data_us <= QUIET_TIMEOUT_US) {
        return;
    }
    finish("the printer sent nothing for 5 s");
    if (in_armed || out_armed) {
        printf("printer: a %s is still outstanding; no new exchange until it completes or the "
               "printer is replugged\n",
               in_armed ? "read" : "write");
    }
}

void printer_link_task(void) {
    if (printer.setup_pending) {
        set_up_printer();
    }
    uint64_t now = time_us_64();
    rearm_if_due(now);
    enforce_quiet_timeout(now);
}

bool printer_link_ready(void) {
    return printer.ready;
}

bool printer_link_busy(void) {
    return exchange.busy;
}

// Why a new exchange cannot start now, or NULL if it can.
static const char *refusal_reason(size_t len) {
    if (!printer.ready) {
        return "the printer is not ready";
    }
    if (exchange.busy) {
        return "an exchange is already running";
    }
    if (in_armed || out_armed) {
        return "a transfer from the previous exchange is still outstanding";
    }
    if (len == 0 || len > UINT16_MAX) {
        return "the request is empty or longer than 65535 bytes";
    }
    return NULL;
}

bool printer_link_start(const uint8_t *request, size_t len, printer_body_fn on_body, void *ctx) {
    const char *refusal = refusal_reason(len);
    if (refusal != NULL) {
        exchange.result = (printer_result_t){.error = refusal};
        return false;
    }
    memset(&exchange, 0, sizeof exchange);
    exchange.busy = true;
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
