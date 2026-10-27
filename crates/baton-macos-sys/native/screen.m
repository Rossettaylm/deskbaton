#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>

#include "deskbaton_native.h"
#include "external_display.h"

typedef struct {
    uint32_t vendor;
    uint32_t model;
    uint32_t serial;
} edid_key;

static uint32_t number(NSDictionary *dict, NSString *key) {
    id value = dict[key];
    return [value isKindOfClass:[NSNumber class]] ? [value unsignedIntValue] : 0;
}

static edid_key key_of(NSDictionary *product) {
    return (edid_key){number(product, @"LegacyManufacturerID"), number(product, @"ProductID"),
                      number(product, @"SerialNumber")};
}

static BOOL same_key(edid_key a, edid_key b) {
    return a.vendor == b.vendor && a.model == b.model && a.serial == b.serial;
}

/* CoreGraphics 与 framebuffer 没有共同 ID，只能按 EDID 的厂商号、产品号、序列号配对；
 * 三者都相同的显示器按出现顺序一一对应。 */
int32_t deskbaton_cg_display(uint32_t display_index, CGDirectDisplayID *out) {
    @autoreleasepool {
        __block edid_key target = {0};
        __block uint32_t rank = 0;
        __block BOOL found = NO;
        NSMutableArray<NSValue *> *earlier = [NSMutableArray array];
        int32_t rc = deskbaton_enumerate_external(^BOOL(uint32_t index, io_service_t av_proxy,
                                                        NSString *edid_uuid, NSDictionary *product) {
            (void)av_proxy;
            (void)edid_uuid;
            edid_key key = key_of(product);
            if (index == display_index) {
                target = key;
                found = YES;
                return NO;
            }
            [earlier addObject:[NSValue valueWithBytes:&key objCType:@encode(edid_key)]];
            return YES;
        });
        if (rc < 0) {
            return rc;
        }
        if (!found) {
            return DESKBATON_ERR_NOT_FOUND;
        }
        for (NSValue *value in earlier) {
            edid_key key;
            [value getValue:&key size:sizeof key];
            if (same_key(key, target)) {
                rank++;
            }
        }

        CGDirectDisplayID ids[32];
        uint32_t count = 0;
        if (CGGetOnlineDisplayList(32, ids, &count) != kCGErrorSuccess) {
            return DESKBATON_ERR_IO;
        }
        for (uint32_t i = 0; i < count; i++) {
            CGDirectDisplayID id = ids[i];
            edid_key key = {CGDisplayVendorNumber(id), CGDisplayModelNumber(id),
                            CGDisplaySerialNumber(id)};
            if (CGDisplayIsBuiltin(id) || !same_key(key, target)) {
                continue;
            }
            if (rank > 0) {
                rank--;
                continue;
            }
            *out = id;
            return DESKBATON_OK;
        }
        return DESKBATON_ERR_NOT_FOUND;
    }
}

int32_t deskbaton_screen_mode(uint32_t display_index, deskbaton_screen_mode_info *out) {
    CGDirectDisplayID id;
    int32_t rc = deskbaton_cg_display(display_index, &id);
    if (rc != DESKBATON_OK) {
        return rc;
    }
    CGDisplayModeRef mode = CGDisplayCopyDisplayMode(id);
    if (mode == NULL) {
        return DESKBATON_ERR_NOT_FOUND;
    }
    out->width = (uint32_t)CGDisplayModeGetWidth(mode);
    out->height = (uint32_t)CGDisplayModeGetHeight(mode);
    out->pixel_width = (uint32_t)CGDisplayModeGetPixelWidth(mode);
    out->pixel_height = (uint32_t)CGDisplayModeGetPixelHeight(mode);
    out->refresh_hz = CGDisplayModeGetRefreshRate(mode);
    CGDisplayModeRelease(mode);
    return DESKBATON_OK;
}
