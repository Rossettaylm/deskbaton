#import <AppKit/AppKit.h>

#include "deskbaton_native.h"

void deskbaton_run_app_loop(void) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        [NSApp run];
    }
}
