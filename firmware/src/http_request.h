// Request encoders for talking to the printer over IPP-over-USB.
#ifndef HTTP_REQUEST_H
#define HTTP_REQUEST_H

#include <stddef.h>
#include <stdint.h>

typedef struct {
    const char *method;
    const char *path;
    const char *content_type; // NULL for a request without a body
    size_t body_len;
} http_head_t;

// Writes an HTTP/1.1 request head the way ipp-usb sends it to USB printers. Returns the head's
// length, or 0 if it does not fit (including its terminating NUL) or if body_len > 0 without a
// content_type. Content-Type and Content-Length are sent together, only when content_type is set.
size_t http_request_head(char *buf, size_t cap, const http_head_t *head);

// Writes an IPP 2.0 Get-Printer-Attributes request for every attribute. Returns its length, or
// 0 if it does not fit.
size_t ipp_get_printer_attributes(uint8_t *buf, size_t cap, uint32_t request_id);

#endif
