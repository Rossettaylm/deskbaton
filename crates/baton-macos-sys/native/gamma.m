#import <CoreGraphics/CoreGraphics.h>

#include "deskbaton_native.h"
#include "external_display.h"

int32_t deskbaton_gamma_get(uint32_t display_index, float *red, float *green, float *blue,
                            uint32_t capacity) {
    CGDirectDisplayID id;
    int32_t rc = deskbaton_cg_display(display_index, &id);
    if (rc != DESKBATON_OK) {
        return rc;
    }
    uint32_t count = 0;
    if (CGGetDisplayTransferByTable(id, capacity, red, green, blue, &count) != kCGErrorSuccess) {
        return DESKBATON_ERR_IO;
    }
    return (int32_t)count;
}

int32_t deskbaton_gamma_set(uint32_t display_index, const float *red, const float *green,
                            const float *blue, uint32_t count) {
    CGDirectDisplayID id;
    int32_t rc = deskbaton_cg_display(display_index, &id);
    if (rc != DESKBATON_OK) {
        return rc;
    }
    return CGSetDisplayTransferByTable(id, count, red, green, blue) == kCGErrorSuccess
               ? DESKBATON_OK
               : DESKBATON_ERR_IO;
}

void deskbaton_gamma_restore_all(void) {
    CGDisplayRestoreColorSyncSettings();
}
