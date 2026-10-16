#import <Foundation/Foundation.h>
#import <IOKit/IOKitLib.h>
#import <IOKit/pwr_mgt/IOPMLib.h>
#import <IOKit/pwr_mgt/IOPMLibDefs.h>

#include "deskbaton_native.h"

static IOPMAssertionID g_idle_assertion = kIOPMNullAssertionID;
static IOPMAssertionID g_user_activity = kIOPMNullAssertionID;

static int32_t set_clamshell_sleep_disabled(uint64_t disabled) {
    io_service_t root =
        IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOPMrootDomain"));
    if (root == IO_OBJECT_NULL) {
        return DESKBATON_ERR_NOT_FOUND;
    }
    io_connect_t conn;
    kern_return_t kr = IOServiceOpen(root, mach_task_self(), 0, &conn);
    IOObjectRelease(root);
    if (kr != KERN_SUCCESS) {
        return DESKBATON_ERR_IO;
    }
    kr = IOConnectCallScalarMethod(conn, kPMSetClamshellSleepState, &disabled, 1, NULL, NULL);
    IOServiceClose(conn);
    return kr == KERN_SUCCESS ? DESKBATON_OK : DESKBATON_ERR_IO;
}

int32_t deskbaton_stay_awake_begin(void) {
    if (g_idle_assertion == kIOPMNullAssertionID &&
        IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleSystemSleep, kIOPMAssertionLevelOn,
                                    CFSTR("DeskBaton stay awake after handoff"),
                                    &g_idle_assertion) != kIOReturnSuccess) {
        g_idle_assertion = kIOPMNullAssertionID;
        return DESKBATON_ERR_IO;
    }
    int32_t rc = set_clamshell_sleep_disabled(1);
    if (rc != DESKBATON_OK) {
        deskbaton_stay_awake_end();
    }
    return rc;
}

int32_t deskbaton_stay_awake_end(void) {
    if (g_idle_assertion != kIOPMNullAssertionID) {
        IOPMAssertionRelease(g_idle_assertion);
        g_idle_assertion = kIOPMNullAssertionID;
    }
    return set_clamshell_sleep_disabled(0);
}

int32_t deskbaton_wake_display(void) {
    IOReturn rc = IOPMAssertionDeclareUserActivity(CFSTR("DeskBaton take over"), kIOPMUserActiveLocal,
                                                   &g_user_activity);
    return rc == kIOReturnSuccess ? DESKBATON_OK : DESKBATON_ERR_IO;
}
