// Finds an interface's bulk endpoint pair in a raw USB configuration descriptor.
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

// The class, subclass and protocol of an interface to look for.
typedef struct {
    uint8_t cls;
    uint8_t subclass;
    uint8_t protocol;
} usb_interface_class_t;

// Finds the first interface (alternate setting 0) with the given class, subclass and protocol
// that has both a bulk OUT and a bulk IN endpoint. Only the first bulk OUT and first bulk IN
// endpoint of that interface are reported. `config` must be a configuration or other-speed
// configuration descriptor. Returns false if there is no such interface, it has no such pair, or
// the descriptor is malformed; `*out` is left untouched in that case.
bool usb_find_bulk_pair(const uint8_t *config, size_t len, usb_interface_class_t wanted,
                        usb_bulk_pair_t *out);

#endif
