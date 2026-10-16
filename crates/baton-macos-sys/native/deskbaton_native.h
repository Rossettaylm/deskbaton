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
/* display_index 为 deskbaton_ddc_list 中的序号；data 为不含主机源地址的 DDC/CI 报文。 */
int32_t deskbaton_ddc_write(uint32_t display_index, const uint8_t *data, uint32_t len);
int32_t deskbaton_ddc_read(uint32_t display_index, uint8_t *out, uint32_t len);

/* 电源：begin 阻止空闲睡眠和合盖睡眠，end 恢复；两者都可重复调用。 */
int32_t deskbaton_stay_awake_begin(void);
int32_t deskbaton_stay_awake_end(void);
/* 声明一次本地用户活动，点亮显示输出。 */
int32_t deskbaton_wake_display(void);

#endif
