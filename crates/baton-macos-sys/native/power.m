#import <Foundation/Foundation.h>
#import <IOKit/IOKitLib.h>
#import <IOKit/pwr_mgt/IOPMLib.h>

#include "deskbaton_native.h"

static IOPMAssertionID g_idle_assertion = kIOPMNullAssertionID;
static IOPMAssertionID g_user_activity = kIOPMNullAssertionID;

int32_t deskbaton_stay_awake_begin(void) {
    if (g_idle_assertion == kIOPMNullAssertionID &&
        IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleSystemSleep, kIOPMAssertionLevelOn,
                                    CFSTR("DeskBaton stay awake after handoff"),
                                    &g_idle_assertion) != kIOReturnSuccess) {
        g_idle_assertion = kIOPMNullAssertionID;
        return DESKBATON_ERR_IO;
    }
    return DESKBATON_OK;
}

int32_t deskbaton_stay_awake_end(void) {
    if (g_idle_assertion != kIOPMNullAssertionID) {
        IOPMAssertionRelease(g_idle_assertion);
        g_idle_assertion = kIOPMNullAssertionID;
    }
    return DESKBATON_OK;
}

int32_t deskbaton_wake_display(void) {
    IOReturn rc = IOPMAssertionDeclareUserActivity(CFSTR("DeskBaton take over"), kIOPMUserActiveLocal,
                                                   &g_user_activity);
    return rc == kIOReturnSuccess ? DESKBATON_OK : DESKBATON_ERR_IO;
}
