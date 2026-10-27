#ifndef DESKBATON_EXTERNAL_DISPLAY_H
#define DESKBATON_EXTERNAL_DISPLAY_H

#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <IOKit/IOKitLib.h>

/* product 为 framebuffer 的 DisplayAttributes.ProductAttributes；返回 NO 停止遍历。 */
typedef BOOL (^ExternalDisplayVisitor)(uint32_t index, io_service_t av_proxy, NSString *edid_uuid,
                                       NSDictionary *product);

/* 按注册表遍历顺序访问外接显示器，返回访问到的台数或负数错误码。 */
int32_t deskbaton_enumerate_external(ExternalDisplayVisitor visit);

/* display_index 对应的 CoreGraphics 显示器；不在在线列表中时返回 NOT_FOUND。 */
int32_t deskbaton_cg_display(uint32_t display_index, CGDirectDisplayID *out);

#endif
