#include "usb_desc.h"

enum {
    DESC_CONFIGURATION = 2,
    DESC_OTHER_SPEED_CONFIGURATION = 7,
    DESC_INTERFACE = 4,
    DESC_ENDPOINT = 5,
    CONFIG_HEADER_LEN = 9,
    INTERFACE_LEN = 9,
    ENDPOINT_LEN = 7,
    ENDPOINT_DIR_IN = 0x80,
    ENDPOINT_TYPE_MASK = 0x03,
    ENDPOINT_TYPE_BULK = 0x02,
    CFG_TOTAL_LENGTH = 2,
    ITF_NUMBER = 2,
    ITF_ALT_SETTING = 3,
    ITF_CLASS = 5,
    ITF_SUBCLASS = 6,
    ITF_PROTOCOL = 7,
    EP_ADDRESS = 2,
    EP_ATTRIBUTES = 3,
    EP_MAX_PACKET = 4,
};

// Validates the configuration (or other-speed configuration) header and returns wTotalLength:
// the number of bytes of `config` that belong to this descriptor set.
static bool config_total_length(const uint8_t *config, size_t len, uint16_t *out) {
    if (len < CONFIG_HEADER_LEN) {
        return false;
    }
    if (config[1] != DESC_CONFIGURATION && config[1] != DESC_OTHER_SPEED_CONFIGURATION) {
        return false;
    }
    uint16_t total = (uint16_t)(config[CFG_TOTAL_LENGTH] | config[CFG_TOTAL_LENGTH + 1] << 8);
    if (total < CONFIG_HEADER_LEN || total > len) {
        return false;
    }
    *out = total;
    return true;
}

static bool matches(const uint8_t *itf, usb_interface_class_t wanted) {
    return itf[ITF_ALT_SETTING] == 0 && itf[ITF_CLASS] == wanted.cls &&
           itf[ITF_SUBCLASS] == wanted.subclass && itf[ITF_PROTOCOL] == wanted.protocol;
}

static bool has_both(const usb_bulk_pair_t *pair) {
    return pair->ep_out != 0 && pair->ep_in != 0;
}

static void note_endpoint(const uint8_t *ep, size_t offset, usb_bulk_pair_t *pair) {
    if ((ep[EP_ATTRIBUTES] & ENDPOINT_TYPE_MASK) != ENDPOINT_TYPE_BULK) {
        return;
    }
    uint16_t size = (uint16_t)(ep[EP_MAX_PACKET] | ep[EP_MAX_PACKET + 1] << 8);
    // Safe: offset < wTotalLength, and wTotalLength is itself a uint16_t (checked by the
    // caller before the walk begins), so these narrowing casts never lose data.
    if ((ep[EP_ADDRESS] & ENDPOINT_DIR_IN) != 0) {
        if (pair->ep_in == 0) {
            pair->ep_in = ep[EP_ADDRESS];
            pair->ep_in_size = size;
            pair->ep_in_offset = (uint16_t)offset;
        }
    } else if (pair->ep_out == 0) {
        pair->ep_out = ep[EP_ADDRESS];
        pair->ep_out_size = size;
        pair->ep_out_offset = (uint16_t)offset;
    }
}

// A malformed descriptor: it claims a length that runs past `total`, or is shorter than its
// type requires.
static bool descriptor_too_short(const uint8_t *desc, size_t offset, uint16_t total) {
    if (desc[0] < 2 || offset + desc[0] > total) {
        return true;
    }
    if (desc[1] == DESC_INTERFACE && desc[0] < INTERFACE_LEN) {
        return true;
    }
    if (desc[1] == DESC_ENDPOINT && desc[0] < ENDPOINT_LEN) {
        return true;
    }
    return false;
}

// Notes a bulk endpoint, but only while walking an interface that matched.
static void note_endpoint_if_inside(const uint8_t *desc, size_t offset, bool inside,
                                    usb_bulk_pair_t *pair) {
    if (inside && desc[1] == DESC_ENDPOINT) {
        note_endpoint(desc, offset, pair);
    }
}

// Starts tracking a new interface. Returns true if the walk should stop here, because the
// interface just finished (still `inside` from the previous iteration) already had both
// endpoints; otherwise resets `pair` and updates `inside` for the interface that is starting.
static bool enter_interface(const uint8_t *itf, bool inside, usb_bulk_pair_t *pair) {
    if (inside && has_both(pair)) {
        return true;
    }
    *pair = (usb_bulk_pair_t){0};
    pair->interface_number = itf[ITF_NUMBER];
    return false;
}

bool usb_find_bulk_pair(const uint8_t *config, size_t len, usb_interface_class_t wanted,
                        usb_bulk_pair_t *out) {
    uint16_t total;
    if (!config_total_length(config, len, &total)) {
        return false;
    }
    usb_bulk_pair_t pair = {0};
    bool inside = false;
    for (size_t offset = 0; offset + 2 <= total;) {
        const uint8_t *desc = config + offset;
        if (descriptor_too_short(desc, offset, total)) {
            return false;
        }
        if (desc[1] == DESC_INTERFACE) {
            if (enter_interface(desc, inside, &pair)) {
                break;
            }
            inside = matches(desc, wanted);
        } else {
            note_endpoint_if_inside(desc, offset, inside, &pair);
        }
        offset += desc[0];
    }
    if (!has_both(&pair)) {
        return false;
    }
    *out = pair;
    return true;
}
