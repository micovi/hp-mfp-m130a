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
// ends) and streams the response body to on_body. Returns false if:
// - the printer is not ready;
// - an exchange is running, or a transfer from an earlier one is still outstanding;
// - len is 0 or more than 65535;
// - the request write could not be queued.
// On false, printer_link_result().error says which; the result describes this attempt until the
// next exchange finishes or is refused.
bool printer_link_start(const uint8_t *request, size_t len, printer_body_fn on_body, void *ctx);

bool printer_link_busy(void);

// The outcome of the last finished exchange, or why the last printer_link_start was refused.
printer_result_t printer_link_result(void);

#endif
