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
// Not a count: TinyUSB indexes its per-device endpoint tables by endpoint number, so this is the
// highest endpoint number + 1. The printer's IPP endpoints go up to 12.
#define CFG_TUH_ENDPOINT_MAX 16
#define CFG_TUH_API_EDPT_XFER 1

#endif
