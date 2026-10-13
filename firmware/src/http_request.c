#include "http_request.h"

#include <stdbool.h>
#include <stdio.h>
#include <string.h>

size_t http_request_head(char *buf, size_t cap, const http_head_t *head) {
    if (head->content_type == NULL && head->body_len > 0) {
        // A body with no declared type is a programming error in this firmware, not a buffer
        // that merely "doesn't fit" -- every body we send has a type.
        return 0;
    }
    int written;
    if (head->content_type == NULL) {
        written = snprintf(buf, cap,
                           "%s %s HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n",
                           head->method, head->path);
    } else {
        written = snprintf(buf, cap,
                           "%s %s HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n"
                           "Content-Type: %s\r\nContent-Length: %zu\r\n\r\n",
                           head->method, head->path, head->content_type, head->body_len);
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
    if (w->overflow || len > w->cap - w->len) {
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

static void put_u32(writer_t *w, uint32_t value) {
    uint8_t bytes[4] = {(uint8_t)(value >> 24), (uint8_t)(value >> 16), (uint8_t)(value >> 8),
                        (uint8_t)value};
    put(w, bytes, sizeof bytes);
}

static void put_attribute(writer_t *w, uint8_t tag, const char *name, const char *value) {
    size_t name_len = strlen(name);
    size_t value_len = strlen(value);
    if (name_len > UINT16_MAX || value_len > UINT16_MAX) {
        w->overflow = true;
        return;
    }
    put_u8(w, tag);
    put_u16(w, (uint16_t)name_len);
    put(w, name, name_len);
    put_u16(w, (uint16_t)value_len);
    put(w, value, value_len);
}

enum {
    IPP_VERSION_MAJOR = 2,
    IPP_VERSION_MINOR = 0,
    IPP_OP_GET_PRINTER_ATTRIBUTES = 0x000B,
    IPP_TAG_OPERATION_ATTRIBUTES = 0x01,
    IPP_TAG_END_OF_ATTRIBUTES = 0x03,
    IPP_TAG_KEYWORD = 0x44,
    IPP_TAG_URI = 0x45,
    IPP_TAG_CHARSET = 0x47,
    IPP_TAG_NATURAL_LANGUAGE = 0x48,
};

size_t ipp_get_printer_attributes(uint8_t *buf, size_t cap, uint32_t request_id) {
    writer_t w = {.buf = buf, .cap = cap};
    put_u8(&w, IPP_VERSION_MAJOR);
    put_u8(&w, IPP_VERSION_MINOR);
    put_u16(&w, IPP_OP_GET_PRINTER_ATTRIBUTES);
    put_u32(&w, request_id);
    put_u8(&w, IPP_TAG_OPERATION_ATTRIBUTES);
    put_attribute(&w, IPP_TAG_CHARSET, "attributes-charset", "utf-8");
    put_attribute(&w, IPP_TAG_NATURAL_LANGUAGE, "attributes-natural-language", "en");
    put_attribute(&w, IPP_TAG_URI, "printer-uri", "ipp://localhost/ipp/print");
    put_attribute(&w, IPP_TAG_KEYWORD, "requested-attributes", "all");
    put_u8(&w, IPP_TAG_END_OF_ATTRIBUTES);
    return w.overflow ? 0 : w.len;
}
