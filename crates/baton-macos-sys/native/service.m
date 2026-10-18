#import "deskbaton_native.h"
#import <ServiceManagement/ServiceManagement.h>
#include <xpc/xpc.h>

int32_t deskbaton_login_item_status(void) {
    SMAppService *svc = [SMAppService mainAppService];
    switch (svc.status) {
        case SMAppServiceStatusNotRegistered:
        case SMAppServiceStatusNotFound:
            return 0;
        case SMAppServiceStatusEnabled:
            return 1;
        case SMAppServiceStatusRequiresApproval:
            return 2;
        default:
            return DESKBATON_ERR_IO;
    }
}

int32_t deskbaton_login_item_set(int32_t enabled) {
    SMAppService *svc = [SMAppService mainAppService];
    NSError *error = nil;
    BOOL ok = enabled ? [svc registerAndReturnError:&error] : [svc unregisterAndReturnError:&error];
    return ok ? DESKBATON_OK : DESKBATON_ERR_IO;
}

int32_t deskbaton_helper_register(void) {
    SMAppService *svc =
        [SMAppService daemonServiceWithPlistName:@"io.github.rossettaylm.deskbaton.sleep.plist"];
    NSError *error = nil;
    if (svc.status == SMAppServiceStatusEnabled || svc.status == SMAppServiceStatusRequiresApproval) {
        return DESKBATON_OK;
    }
    return [svc registerAndReturnError:&error] ? DESKBATON_OK : DESKBATON_ERR_IO;
}

int32_t deskbaton_helper_set_sleep_disabled(int32_t disabled) {
    xpc_connection_t conn = xpc_connection_create_mach_service(
        "io.github.rossettaylm.deskbaton.sleep", NULL, XPC_CONNECTION_MACH_SERVICE_PRIVILEGED);
    if (!conn) {
        return DESKBATON_ERR_IO;
    }
    xpc_connection_set_event_handler(conn, ^(xpc_object_t event) {
        (void)event;
    });
    xpc_connection_resume(conn);
    xpc_object_t req = xpc_dictionary_create(NULL, NULL, 0);
    xpc_dictionary_set_int64(req, "disablesleep", disabled ? 1 : 0);
    xpc_object_t reply = xpc_connection_send_message_with_reply_sync(conn, req);
    int32_t rc = DESKBATON_ERR_IO;
    if (reply && xpc_get_type(reply) == XPC_TYPE_DICTIONARY) {
        rc = (int32_t)xpc_dictionary_get_int64(reply, "rc");
    }
    return rc;
}
