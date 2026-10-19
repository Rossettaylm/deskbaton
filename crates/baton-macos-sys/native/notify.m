#import <AppKit/AppKit.h>

#include "deskbaton_native.h"

static NSString *const kRevealKey = @"reveal";

@interface DBNotifyDelegate : NSObject <NSUserNotificationCenterDelegate>
@end

@implementation DBNotifyDelegate
- (void)userNotificationCenter:(NSUserNotificationCenter *)center
       didActivateNotification:(NSUserNotification *)notification {
    NSString *path = notification.userInfo[kRevealKey];
    if (path.length > 0) {
        [[NSWorkspace sharedWorkspace]
            activateFileViewerSelectingURLs:@[ [NSURL fileURLWithPath:path] ]];
    }
}
@end

static void deliver(const char *message, NSString *reveal) {
    static DBNotifyDelegate *delegate;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        delegate = [DBNotifyDelegate new];
        [NSUserNotificationCenter defaultUserNotificationCenter].delegate = delegate;
    });
    NSUserNotification *note = [NSUserNotification new];
    note.title = @"DeskBaton";
    note.informativeText = [NSString stringWithUTF8String:message];
    if (reveal != nil) {
        note.userInfo = @{kRevealKey : reveal};
    }
    [[NSUserNotificationCenter defaultUserNotificationCenter] deliverNotification:note];
}

int32_t deskbaton_notify(const char *message) {
    @autoreleasepool {
        if (message == NULL) {
            return DESKBATON_ERR_IO;
        }
        deliver(message, nil);
        return DESKBATON_OK;
    }
}

int32_t deskbaton_notify_reveal(const char *message, const char *path) {
    @autoreleasepool {
        if (message == NULL || path == NULL) {
            return DESKBATON_ERR_IO;
        }
        deliver(message, [NSString stringWithUTF8String:path]);
        return DESKBATON_OK;
    }
}
