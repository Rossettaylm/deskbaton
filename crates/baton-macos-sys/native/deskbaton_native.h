#ifndef DESKBATON_NATIVE_H
#define DESKBATON_NATIVE_H

#include <stdint.h>

/* 返回值约定：0 成功，负数为错误码。 */
#define DESKBATON_OK 0
#define DESKBATON_ERR_UNIMPLEMENTED (-1)

/* DDC/CI（Apple Silicon，经 IOAVService） */
int32_t deskbaton_ddc_display_count(void);
int32_t deskbaton_ddc_get_input(uint32_t display_index, uint16_t *out_input);
int32_t deskbaton_ddc_set_input(uint32_t display_index, uint16_t input);

/* 电源：在 seconds 秒内阻止系统睡眠（含合盖睡眠），end 立即恢复 */
int32_t deskbaton_stay_awake_begin(uint32_t seconds);
int32_t deskbaton_stay_awake_end(void);
int32_t deskbaton_wake_display(void);

#endif
