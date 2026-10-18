#include <dispatch/dispatch.h>
#include <spawn.h>
#include <sys/wait.h>
#include <xpc/xpc.h>

extern char **environ;

static int run_pmset(int disabled) {
    pid_t pid = 0;
    const char *argv[] = {"/usr/bin/pmset", "-a", "disablesleep", disabled ? "1" : "0", NULL};
    if (posix_spawn(&pid, argv[0], NULL, NULL, (char *const *)argv, environ) != 0) {
        return -1;
    }
    int status = 0;
    if (waitpid(pid, &status, 0) < 0) {
        return -1;
    }
    return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

static void handle_request(xpc_object_t event, xpc_connection_t peer) {
    if (xpc_get_type(event) != XPC_TYPE_DICTIONARY) {
        return;
    }
    int64_t disabled = xpc_dictionary_get_int64(event, "disablesleep");
    if (disabled != 0 && disabled != 1) {
        xpc_object_t reply = xpc_dictionary_create_reply(event);
        if (reply) {
            xpc_dictionary_set_int64(reply, "rc", -3);
            xpc_connection_send_message(peer, reply);
        }
        return;
    }
    int rc = run_pmset((int)disabled);
    xpc_object_t reply = xpc_dictionary_create_reply(event);
    if (reply) {
        xpc_dictionary_set_int64(reply, "rc", rc);
        xpc_connection_send_message(peer, reply);
    }
}

void deskbaton_sleep_helper_serve(void) {
    xpc_connection_t service = xpc_connection_create_mach_service(
        "io.github.rossettaylm.deskbaton.sleep", dispatch_get_main_queue(),
        XPC_CONNECTION_MACH_SERVICE_LISTENER);
    if (!service) {
        return;
    }
    xpc_connection_set_event_handler(service, ^(xpc_object_t peer_object) {
        if (xpc_get_type(peer_object) != XPC_TYPE_CONNECTION) {
            return;
        }
        xpc_connection_t peer = (xpc_connection_t)peer_object;
        xpc_connection_set_event_handler(peer, ^(xpc_object_t event) {
            handle_request(event, peer);
        });
        xpc_connection_resume(peer);
    });
    xpc_connection_resume(service);
    dispatch_main();
}
