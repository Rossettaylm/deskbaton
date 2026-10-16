#import <Foundation/Foundation.h>
#import <IOKit/IOKitLib.h>
#include <string.h>

#include "deskbaton_native.h"
#include "external_display.h"

typedef CFTypeRef IOAVServiceRef;
extern IOAVServiceRef IOAVServiceCreateWithService(CFAllocatorRef allocator, io_service_t service);
extern IOReturn IOAVServiceReadI2C(IOAVServiceRef service, uint32_t chip_address, uint32_t offset,
                                   void *out, uint32_t len);
extern IOReturn IOAVServiceWriteI2C(IOAVServiceRef service, uint32_t chip_address, uint32_t offset,
                                    void *data, uint32_t len);

static const uint32_t kDdcChipAddress = 0x37;
static const uint32_t kDdcHostAddress = 0x51;

static id copy_property(io_registry_entry_t entry, NSString *key, Class expected) {
    id value = CFBridgingRelease(
        IORegistryEntryCreateCFProperty(entry, (__bridge CFStringRef)key, kCFAllocatorDefault, 0));
    return [value isKindOfClass:expected] ? value : nil;
}

static BOOL is_framebuffer(const char *name) {
    return strcmp(name, "AppleCLCD2") == 0 || strcmp(name, "IOMobileFramebufferShim") == 0;
}

/* 每台外接显示器在注册表遍历顺序中先出现 framebuffer（带 EDID UUID），
 * 后出现对应的 DCPAVServiceProxy；没有接显示器的 framebuffer 没有 EDID UUID。 */
int32_t deskbaton_enumerate_external(ExternalDisplayVisitor visit) {
    io_iterator_t iter;
    if (IORegistryEntryCreateIterator(IORegistryGetRootEntry(kIOMainPortDefault), kIOServicePlane,
                                      kIORegistryIterateRecursively, &iter) != KERN_SUCCESS) {
        return DESKBATON_ERR_IO;
    }
    NSString *edid_uuid = nil;
    NSDictionary *product = nil;
    uint32_t index = 0;
    io_service_t entry;
    while ((entry = IOIteratorNext(iter)) != IO_OBJECT_NULL) {
        BOOL keep_going = YES;
        io_name_t name;
        if (IORegistryEntryGetName(entry, name) == KERN_SUCCESS) {
            if (is_framebuffer(name)) {
                edid_uuid = copy_property(entry, @"EDID UUID", [NSString class]);
                NSDictionary *attrs = copy_property(entry, @"DisplayAttributes", [NSDictionary class]);
                product = attrs[@"ProductAttributes"];
            } else if (strcmp(name, "DCPAVServiceProxy") == 0) {
                NSString *location = copy_property(entry, @"Location", [NSString class]);
                if (edid_uuid != nil && [location isEqualToString:@"External"]) {
                    keep_going = visit(index++, entry, edid_uuid, product);
                }
                edid_uuid = nil;
                product = nil;
            }
        }
        IOObjectRelease(entry);
        if (!keep_going) {
            break;
        }
    }
    IOObjectRelease(iter);
    return (int32_t)index;
}

static void copy_text(char *dst, size_t size, id value) {
    NSString *text = [value isKindOfClass:[NSNumber class]] ? [value stringValue] : value;
    if (![text isKindOfClass:[NSString class]]) {
        text = @"";
    }
    strlcpy(dst, text.UTF8String, size);
}

int32_t deskbaton_ddc_list(deskbaton_display_info *out, uint32_t capacity) {
    @autoreleasepool {
        return deskbaton_enumerate_external(^BOOL(uint32_t index, io_service_t av_proxy, NSString *edid_uuid,
                                        NSDictionary *product) {
            (void)av_proxy;
            if (index < capacity) {
                deskbaton_display_info *info = &out[index];
                copy_text(info->edid_uuid, sizeof info->edid_uuid, edid_uuid);
                copy_text(info->vendor, sizeof info->vendor, product[@"ManufacturerID"]);
                copy_text(info->model, sizeof info->model, product[@"ProductName"]);
                copy_text(info->serial, sizeof info->serial,
                          product[@"AlphanumericSerialNumber"] ?: product[@"SerialNumber"]);
            }
            return YES;
        });
    }
}

static int32_t with_av_service(uint32_t display_index, IOReturn (^op)(IOAVServiceRef service)) {
    @autoreleasepool {
        __block int32_t rc = DESKBATON_ERR_NOT_FOUND;
        int32_t count = deskbaton_enumerate_external(^BOOL(uint32_t index, io_service_t av_proxy,
                                                 NSString *edid_uuid, NSDictionary *product) {
            (void)edid_uuid;
            (void)product;
            if (index != display_index) {
                return YES;
            }
            IOAVServiceRef service = IOAVServiceCreateWithService(kCFAllocatorDefault, av_proxy);
            if (service == NULL) {
                rc = DESKBATON_ERR_IO;
                return NO;
            }
            rc = op(service) == kIOReturnSuccess ? DESKBATON_OK : DESKBATON_ERR_IO;
            CFRelease(service);
            return NO;
        });
        return count < 0 ? count : rc;
    }
}

int32_t deskbaton_ddc_write(uint32_t display_index, const uint8_t *data, uint32_t len) {
    return with_av_service(display_index, ^IOReturn(IOAVServiceRef service) {
        return IOAVServiceWriteI2C(service, kDdcChipAddress, kDdcHostAddress, (void *)data, len);
    });
}

int32_t deskbaton_ddc_read(uint32_t display_index, uint8_t *out, uint32_t len) {
    return with_av_service(display_index, ^IOReturn(IOAVServiceRef service) {
        return IOAVServiceReadI2C(service, kDdcChipAddress, kDdcHostAddress, out, len);
    });
}
