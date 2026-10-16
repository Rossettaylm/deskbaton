#ifndef DESKBATON_NATIVE_H
#define DESKBATON_NATIVE_H

#include <stdint.h>

/* 返回值约定：0 成功，负数为错误码。 */
#define DESKBATON_OK 0
#define DESKBATON_ERR_UNIMPLEMENTED (-1)
#define DESKBATON_ERR_NOT_FOUND (-2)
#define DESKBATON_ERR_IO (-3)

/* DDC/CI（Apple Silicon，经 IOAVService） */
typedef struct {
    char edid_uuid[64];
    char vendor[16];
    char model[64];
    char serial[64];
} deskbaton_display_info;

/* 枚举外接显示器，最多写入 capacity 项；返回实际台数（可能大于 capacity）。 */
int32_t deskbaton_ddc_list(deskbaton_display_info *out, uint32_t capacity);
/* display_index 为 deskbaton_ddc_list 中的序号；data 为不含主机源地址的 DDC/CI 报文。
 * 同一报文连发 write_cycles 次，每次前等 10 毫秒。 */
int32_t deskbaton_ddc_write(uint32_t display_index, const uint8_t *data, uint32_t len,
                            uint32_t write_cycles);
/* 在同一 IOAVService 上按 deskbaton_ddc_write 的方式发请求，等 reply_delay_ms 后读一次回包。 */
int32_t deskbaton_ddc_transact(uint32_t display_index, const uint8_t *data, uint32_t data_len,
                               uint32_t write_cycles, uint32_t reply_delay_ms, uint8_t *out,
                               uint32_t out_len);

/* 屏幕当前模式；width / height 为逻辑分辨率，pixel_* 为渲染分辨率，refresh_hz 为 0 表示未知。 */
typedef struct {
    uint32_t width;
    uint32_t height;
    uint32_t pixel_width;
    uint32_t pixel_height;
    double refresh_hz;
} deskbaton_screen_mode_info;

/* display_index 同 deskbaton_ddc_list；显示器不在 CoreGraphics 的在线列表中时返回 NOT_FOUND。 */
int32_t deskbaton_screen_mode(uint32_t display_index, deskbaton_screen_mode_info *out);

/* 电源：begin 持有防空闲睡眠断言，end 释放；两者都可重复调用。合盖睡眠由 Rust 侧经 pmset 控制。 */
int32_t deskbaton_stay_awake_begin(void);
int32_t deskbaton_stay_awake_end(void);
/* 声明一次本地用户活动，点亮显示输出。 */
int32_t deskbaton_wake_display(void);

/* 在主线程运行 NSApplication 事件循环（不显示 Dock 图标），全局热键依赖它分发事件；不返回。 */
void deskbaton_run_app_loop(void);

#endif
