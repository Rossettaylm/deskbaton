#include "deskbaton_native.h"

int32_t deskbaton_ddc_display_count(void) {
    return DESKBATON_ERR_UNIMPLEMENTED;
}

int32_t deskbaton_ddc_get_input(uint32_t display_index, uint16_t *out_input) {
    (void)display_index;
    (void)out_input;
    return DESKBATON_ERR_UNIMPLEMENTED;
}

int32_t deskbaton_ddc_set_input(uint32_t display_index, uint16_t input) {
    (void)display_index;
    (void)input;
    return DESKBATON_ERR_UNIMPLEMENTED;
}
