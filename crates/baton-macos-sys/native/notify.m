#import <AppKit/AppKit.h>

#include "deskbaton_native.h"

int32_t deskbaton_notify(const char *message) {
    @autoreleasepool {
        if (message == NULL) {
            return DESKBATON_ERR_IO;
        }
        NSUserNotification *note = [NSUserNotification new];
        note.title = @"DeskBaton";
        note.informativeText = [NSString stringWithUTF8String:message];
        [[NSUserNotificationCenter defaultUserNotificationCenter] deliverNotification:note];
        return DESKBATON_OK;
    }
}
