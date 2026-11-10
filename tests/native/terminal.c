#include <ghostty/vt.h>
#include <assert.h>
#include <stdio.h>
#include <string.h>

typedef struct { uint8_t bytes[16384]; size_t length; } Output;
static bool capture(void *context, const uint8_t *bytes, size_t length) {
    Output *output = context;
    if (length >= sizeof(output->bytes) - output->length) return false;
    memcpy(output->bytes + output->length, bytes, length); output->length += length; output->bytes[output->length] = 0;
    return true;
}
static void reply(GhosttyTerminal terminal, void *context, const uint8_t *bytes, size_t length) { (void)terminal; assert(capture(context, bytes, length)); }
static void write_vt(GhosttyTerminal terminal, const char *text) { ghostty_terminal_vt_write(terminal, (const uint8_t *)text, strlen(text)); }
static bool paste_text(void *context, GhosttyString mime, GhosttyWriter writer) { (void)mime; const char *text = context; return writer.write(writer.userdata, (const uint8_t *)text, strlen(text)); }
static GhosttyPoint point(uint16_t x, uint32_t y) { return (GhosttyPoint){.tag=GHOSTTY_POINT_TAG_VIEWPORT, .value={.coordinate={.x=x,.y=y}}}; }
static void format_selection(GhosttyTerminal terminal, Output *output) {
    GhosttySelection selection = {0}; assert(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_SELECTION, &selection) == GHOSTTY_SUCCESS);
    GhosttyFormatter formatter = NULL; assert(ghostty_formatter_terminal_new(NULL, &formatter, terminal) == GHOSTTY_SUCCESS);
    assert(ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_SELECTION, &selection) == GHOSTTY_SUCCESS);
    bool unwrap = true; ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_UNWRAP, &unwrap);
    assert(ghostty_formatter_format(formatter, (GhosttyWriter){.write=capture,.userdata=output}) == GHOSTTY_SUCCESS);
    ghostty_formatter_free(formatter);
}
int main(void) {
    GhosttyTerminal terminal = NULL; assert(ghostty_terminal_new(NULL, &terminal, 80, 24) == GHOSTTY_SUCCESS);
    Output output = {0}; ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_USERDATA, &output); ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_WRITE_PTY, (const void *)reply);
    write_vt(terminal, "Merhaba Türkçe: ğüşiöç İĞÜŞÖÇ e\xcc\x81 日本語");
    uint16_t x = 0; assert(ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_X, &x) == GHOSTTY_SUCCESS); assert(x > 30 && x < 50);
    write_vt(terminal, "\x1b[6n"); assert(output.length > 3 && output.bytes[0] == 27 && output.bytes[output.length-1] == 'R');
    write_vt(terminal, "\x1b[?1049h\x1b[Heditor");
    GhosttyTerminalScreen screen; ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen); assert(screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE);
    write_vt(terminal, "\x1b[?1049l"); ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen); assert(screen == GHOSTTY_TERMINAL_SCREEN_PRIMARY);
    GhosttyKeyEncoder encoder = NULL; GhosttyKeyEvent event = NULL;
    assert(ghostty_key_encoder_new(NULL, &encoder) == GHOSTTY_SUCCESS); assert(ghostty_key_event_new(NULL, &event) == GHOSTTY_SUCCESS);
    ghostty_key_event_set_action(event, GHOSTTY_KEY_ACTION_PRESS); ghostty_key_event_set_key(event, GHOSTTY_KEY_ARROW_UP);
    uint8_t encoded[64]; size_t length = 0;
    write_vt(terminal, "\x1b[?1l"); ghostty_key_encoder_set_from_terminal(encoder, terminal); assert(ghostty_key_encoder_encode(encoder,event,encoded,sizeof(encoded),&length) == GHOSTTY_SUCCESS); assert(length == 3 && memcmp(encoded,"\x1b[A",3) == 0);
    write_vt(terminal, "\x1b[?1h"); ghostty_key_encoder_set_from_terminal(encoder, terminal); ghostty_key_encoder_encode(encoder,event,encoded,sizeof(encoded),&length); assert(length == 3 && memcmp(encoded,"\x1bOA",3) == 0);
    output.length = 0; write_vt(terminal, "\x1b[?2004h");
    GhosttyString mime = {.ptr=(const uint8_t *)"text/plain",.len=10}; GhosttyPaste paste = GHOSTTY_INIT_SIZED(GhosttyPaste);
    paste.location=GHOSTTY_CLIPBOARD_LOCATION_STANDARD; paste.source=GHOSTTY_PASTE_SOURCE_CLIPBOARD; paste.mimes=&mime; paste.mimes_len=1; paste.allow_unsafe=true; paste.reader=(GhosttyMimeReader){.read=paste_text,.userdata="Türkçe\nikinci satır"};
    assert(ghostty_terminal_paste(terminal,&paste,NULL) == GHOSTTY_SUCCESS); assert(memcmp(output.bytes,"\x1b[200~",6) == 0); assert(memcmp(output.bytes+output.length-6,"\x1b[201~",6) == 0);
    ghostty_terminal_reset(terminal); write_vt(terminal,"İstanbul is ready");
    GhosttySelection selection = {0}; ghostty_terminal_grid_ref(terminal,point(0,0),&selection.start); ghostty_terminal_grid_ref(terminal,point(7,0),&selection.end); ghostty_terminal_set(terminal,GHOSTTY_TERMINAL_OPT_SELECTION,&selection);
    assert(ghostty_terminal_resize(terminal,12,6,16,32) == GHOSTTY_SUCCESS); output.length=0; format_selection(terminal,&output); assert(strcmp((const char *)output.bytes,"İstanbul") == 0);
    for (int i=0;i<1000;i++) write_vt(terminal,"history row\r\n");
    GhosttyTerminalScrollViewport scroll={.tag=GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP}; ghostty_terminal_scroll_viewport(terminal,scroll); bool active=true; ghostty_terminal_get(terminal,GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE,&active); assert(!active);
    scroll.tag=GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM; ghostty_terminal_scroll_viewport(terminal,scroll); ghostty_terminal_get(terminal,GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE,&active); assert(active);
    ghostty_key_event_free(event); ghostty_key_encoder_free(encoder); ghostty_terminal_free(terminal);
    puts("Terminal engine: Unicode, replies, alternate screen, cursor keys, paste, selection/reflow and scrollback passed.");
    return 0;
}
