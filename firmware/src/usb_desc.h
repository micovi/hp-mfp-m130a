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
