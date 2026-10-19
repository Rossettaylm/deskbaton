#import <AppKit/AppKit.h>

#include "deskbaton_native.h"

static deskbaton_send_cb g_send_cb;

@interface DBSendService : NSObject
@end

@implementation DBSendService
/* 选择器须与 Info.plist 里 NSServices 的 NSMessage 一致。 */
- (void)sendFiles:(NSPasteboard *)pboard
         userData:(NSString *)userData
            error:(NSString *__autoreleasing *)error {
    NSArray<NSURL *> *urls =
        [pboard readObjectsForClasses:@[ NSURL.class ]
                              options:@{NSPasteboardURLReadingFileURLsOnlyKey : @YES}];
    if (urls.count == 0 || g_send_cb == NULL) {
        if (error != NULL) {
            *error = @"没有可发送的文件";
        }
        return;
    }
    const char **paths = calloc(urls.count, sizeof(char *));
    if (paths == NULL) {
        return;
    }
    for (NSUInteger i = 0; i < urls.count; i++) {
        paths[i] = urls[i].fileSystemRepresentation;
    }
    g_send_cb(paths, (uint32_t)urls.count);
    free(paths);
}
@end

int32_t deskbaton_register_send_service(deskbaton_send_cb cb) {
    if (cb == NULL || NSApp == nil) {
        return DESKBATON_ERR_IO;
    }
    g_send_cb = cb;
    static DBSendService *provider;
    if (provider == nil) {
        provider = [DBSendService new];
    }
    [NSApp setServicesProvider:provider];
    NSUpdateDynamicServices();
    return DESKBATON_OK;
}
