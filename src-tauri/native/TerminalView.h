#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

typedef void (*BrigTerminalInput)(uint64_t, const uint8_t *, size_t);
typedef void (*BrigTerminalResize)(uint64_t, uint16_t, uint16_t, uint16_t, uint16_t);
void *brig_terminal_create(void *window, uint64_t identifier, BrigTerminalInput input, BrigTerminalResize resize);
void brig_terminal_layout(void *view, double x, double y, double width, double height, bool visible);
void brig_terminal_feed(void *view, const uint8_t *data, size_t length);
void brig_terminal_focus(void *view);
void brig_terminal_show(void *view, bool visible);
void brig_terminal_destroy(void *view);
void brig_terminal_copy_text(const uint8_t *bytes, size_t length);
