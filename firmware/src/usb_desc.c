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
