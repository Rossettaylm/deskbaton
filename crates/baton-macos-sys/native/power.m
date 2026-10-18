#import <CoreFoundation/CoreFoundation.h>
#import <Foundation/Foundation.h>
#import <IOKit/IOKitLib.h>
#import <IOKit/IOMessage.h>
#import <IOKit/pwr_mgt/IOPMLib.h>

#include "deskbaton_native.h"

static IOPMAssertionID g_idle_assertion = kIOPMNullAssertionID;
static IOPMAssertionID g_display_assertion = kIOPMNullAssertionID;
static IOPMAssertionID g_user_activity = kIOPMNullAssertionID;
static io_connect_t g_root_port = MACH_PORT_NULL;
static deskbaton_wake_cb g_wake_cb;

static void release_assertion(IOPMAssertionID *id) {
    if (*id != kIOPMNullAssertionID) {
        IOPMAssertionRelease(*id);
        *id = kIOPMNullAssertionID;
    }
}

int32_t deskbaton_stay_awake_begin(void) {
    if (g_idle_assertion == kIOPMNullAssertionID &&
        IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleSystemSleep, kIOPMAssertionLevelOn,
                                    CFSTR("DeskBaton stay awake"),
                                    &g_idle_assertion) != kIOReturnSuccess) {
        g_idle_assertion = kIOPMNullAssertionID;
        return DESKBATON_ERR_IO;
    }
    if (g_display_assertion == kIOPMNullAssertionID &&
        IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleDisplaySleep, kIOPMAssertionLevelOn,
                                    CFSTR("DeskBaton stay awake"),
                                    &g_display_assertion) != kIOReturnSuccess) {
        release_assertion(&g_idle_assertion);
        return DESKBATON_ERR_IO;
    }
    return DESKBATON_OK;
}

int32_t deskbaton_stay_awake_end(void) {
    release_assertion(&g_display_assertion);
    release_assertion(&g_idle_assertion);
    return DESKBATON_OK;
}

int32_t deskbaton_wake_display(void) {
    IOReturn rc = IOPMAssertionDeclareUserActivity(CFSTR("DeskBaton take over"), kIOPMUserActiveLocal,
                                                   &g_user_activity);
    return rc == kIOReturnSuccess ? DESKBATON_OK : DESKBATON_ERR_IO;
}

static void power_callback(void *refcon, io_service_t service, uint32_t messageType, void *arg) {
    (void)refcon;
    (void)service;
    switch (messageType) {
        case kIOMessageCanSystemSleep:
        case kIOMessageSystemWillSleep:
            IOAllowPowerChange(g_root_port, (long)arg);
            break;
        case kIOMessageSystemHasPoweredOn:
            if (g_wake_cb) {
                g_wake_cb();
            }
            break;
        default:
            break;
    }
}

int32_t deskbaton_watch_wake(deskbaton_wake_cb cb) {
    g_wake_cb = cb;
    IONotificationPortRef notify = NULL;
    io_object_t notifier = IO_OBJECT_NULL;
    g_root_port = IORegisterForSystemPower(NULL, &notify, power_callback, &notifier);
    if (g_root_port == MACH_PORT_NULL || notify == NULL) {
        return DESKBATON_ERR_IO;
    }
    CFRunLoopAddSource(CFRunLoopGetCurrent(), IONotificationPortGetRunLoopSource(notify),
                       kCFRunLoopDefaultMode);
    CFRunLoopRun();
    return DESKBATON_OK;
}
