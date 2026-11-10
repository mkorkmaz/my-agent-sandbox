#include "TerminalViewPrivate.h"

static void terminalReply(GhosttyTerminal ignored, void *context, const uint8_t *data, size_t length) {
    (void)ignored;
    BrigTerminalView *view = (__bridge BrigTerminalView *)context;
    if (view->input) view->input(view->identifier, data, length);
}

NSFont *brigTerminalFont(NSString *preferredName, NSURL *fallbackURL) {
    NSFont *preferred = [NSFont fontWithName:preferredName size:BrigTerminalFontSize];
    if (preferred) return preferred;
    NSFont *fallback = [NSFont fontWithName:@"JetBrainsMono-Regular" size:BrigTerminalFontSize];
    if (fallback) return fallback;
    if (!fallbackURL) return nil;
    CFErrorRef error = NULL;
    BOOL registered = CTFontManagerRegisterFontsForURL((__bridge CFURLRef)fallbackURL, kCTFontManagerScopeProcess, &error);
    if (error) CFRelease(error);
    if (!registered) return nil;
    return [NSFont fontWithName:@"JetBrainsMono-Regular" size:BrigTerminalFontSize];
}

@implementation BrigTerminalView
- (BOOL)isFlipped { return YES; }
- (BOOL)isOpaque { return YES; }
- (BOOL)acceptsFirstResponder { return YES; }
- (BOOL)becomeFirstResponder { [self setNeedsDisplay:YES]; return YES; }
- (BOOL)resignFirstResponder { [self setNeedsDisplay:YES]; return YES; }
- (void)dealloc {
    [cursorTimer invalidate];
    [dragTimer invalidate];
    ghostty_tracked_grid_ref_free(selectionAnchor);
    ghostty_formatter_free(formatter);
    ghostty_key_event_free(keyEvent);
    ghostty_key_encoder_free(keyEncoder);
    ghostty_render_state_row_cells_free(cells);
    ghostty_render_state_row_iterator_free(rowIterator);
    ghostty_render_state_free(renderState);
    ghostty_terminal_free(terminal);
}
- (void)updateGrid {
    if (!terminal) return;
    uint16_t nextColumns = MAX(2, MIN(1000, floor((self.bounds.size.width - BrigTerminalPaddingX * 2) / cellWidth)));
    uint16_t nextRows = MAX(2, MIN(1000, floor((self.bounds.size.height - BrigTerminalPaddingY * 2) / cellHeight)));
    CGFloat scale = self.window.backingScaleFactor ?: 2;
    uint16_t nextPixelWidth = MIN(UINT16_MAX, round(nextColumns * cellWidth * scale));
    uint16_t nextPixelHeight = MIN(UINT16_MAX, round(nextRows * cellHeight * scale));
    if (columns == nextColumns && rows == nextRows && pixelWidth == nextPixelWidth && pixelHeight == nextPixelHeight) return;
    columns = nextColumns; rows = nextRows; pixelWidth = nextPixelWidth; pixelHeight = nextPixelHeight;
    ghostty_terminal_resize(terminal, columns, rows, round(cellWidth * scale), round(cellHeight * scale));
    if (resize) resize(identifier, columns, rows, pixelWidth, pixelHeight);
    [self setNeedsDisplay:YES];
}
- (void)setFrameSize:(NSSize)size { [super setFrameSize:size]; [self updateGrid]; }
- (void)viewDidChangeBackingProperties { [super viewDidChangeBackingProperties]; [self updateGrid]; }
- (void)viewDidChangeEffectiveAppearance {
    [super viewDidChangeEffectiveAppearance];
    if (!terminal) return;
    GhosttyColorRgb background = brigTerminalRGB(BrigTerminalBackground);
    GhosttyColorRgb foreground = brigTerminalRGB(BrigTerminalForeground);
    GhosttyColorRgb cursor = brigTerminalRGB(BrigTerminalCursor);
    ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &background);
    ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &foreground);
    ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, &cursor);
    [glyphCache removeAllObjects];
    [self setNeedsDisplay:YES];
}
@end

void *brig_terminal_create(void *windowPointer, uint64_t session, BrigTerminalInput onInput, BrigTerminalResize onResize) {
    NSCAssert(NSThread.isMainThread, @"Terminal view must be created on the main thread");
    BrigTerminalView *view = [[BrigTerminalView alloc] initWithFrame:NSZeroRect];
    view->identifier = session; view->input = onInput; view->resize = onResize;
    NSURL *fallbackURL = [NSBundle.mainBundle URLForResource:@"JetBrainsMono-Variable" withExtension:@"ttf" subdirectory:@"fonts"];
    view->font = brigTerminalFont(@"Consolas", fallbackURL);
    if (!view->font) return NULL;
    view->cellWidth = ceil([@"M" sizeWithAttributes:@{NSFontAttributeName:view->font}].width * 2) / 2;
    view->cellHeight = ceil((view->font.ascender - view->font.descender + view->font.leading) * BrigTerminalCellHeightScale);
    view->markedText = [[NSMutableAttributedString alloc] init];
    view->glyphCache = [[NSCache alloc] init];
    view->glyphCache.countLimit = 2048;
    view->glyphCache.totalCostLimit = 4 * 1024 * 1024;
    if (ghostty_terminal_new(NULL, &view->terminal, 80, 24) != GHOSTTY_SUCCESS ||
        ghostty_render_state_new(NULL, &view->renderState) != GHOSTTY_SUCCESS ||
        ghostty_render_state_row_iterator_new(NULL, &view->rowIterator) != GHOSTTY_SUCCESS ||
        ghostty_render_state_row_cells_new(NULL, &view->cells) != GHOSTTY_SUCCESS ||
        ghostty_key_encoder_new(NULL, &view->keyEncoder) != GHOSTTY_SUCCESS ||
        ghostty_key_event_new(NULL, &view->keyEvent) != GHOSTTY_SUCCESS ||
        ghostty_formatter_terminal_new(NULL, &view->formatter, view->terminal) != GHOSTTY_SUCCESS) return NULL;
    size_t historyBytes = 32 * 1024 * 1024;
    size_t historyLines = 10000;
    ghostty_terminal_set(view->terminal, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, &historyBytes);
    ghostty_terminal_set(view->terminal, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &historyLines);
    ghostty_terminal_set(view->terminal, GHOSTTY_TERMINAL_OPT_USERDATA, (__bridge void *)view);
    ghostty_terminal_set(view->terminal, GHOSTTY_TERMINAL_OPT_WRITE_PTY, (void *)terminalReply);
    GhosttyColorRgb palette[256];
    ghostty_terminal_get(view->terminal, GHOSTTY_TERMINAL_DATA_COLOR_PALETTE_DEFAULT, &palette);
    for (NSUInteger index = 0; index < 16; index++) palette[index] = brigTerminalRGB(BrigTerminalPalette[index]);
    ghostty_terminal_set(view->terminal, GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, &palette);
    [view setAccessibilityElement:YES];
    [view setAccessibilityRole:NSAccessibilityTextAreaRole];
    [view setAccessibilityLabel:@"Terminal"];
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    view.appearance = [NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
    [window.contentView addSubview:view positioned:NSWindowAbove relativeTo:nil];
    [view viewDidChangeEffectiveAppearance];
    __weak BrigTerminalView *weakView = view;
    view->cursorTimer = [NSTimer scheduledTimerWithTimeInterval:0.5 repeats:YES block:^(NSTimer *timer) {
        BrigTerminalView *current = weakView;
        if (!current) { [timer invalidate]; return; }
        if (!current.hidden && current.window.isKeyWindow) [current setNeedsDisplay:YES];
    }];
    return (__bridge_retained void *)view;
}
void brig_terminal_layout(void *pointer, double x, double y, double width, double height, bool visible) {
    BrigTerminalView *view = (__bridge BrigTerminalView *)pointer;
    NSView *parent = view.superview;
    NSRect content = [parent convertRect:view.window.contentLayoutRect fromView:nil];
    double originY = parent.isFlipped ? content.origin.y + y : NSMaxY(content) - y - height;
    if (width >= 20 && height >= 20) view.frame = NSMakeRect(content.origin.x + x, originY, width, height);
    view.hidden = !visible;
}
void brig_terminal_feed(void *pointer, const uint8_t *data, size_t length) {
    BrigTerminalView *view = (__bridge BrigTerminalView *)pointer;
    ghostty_terminal_vt_write(view->terminal, data, length);
    [view setNeedsDisplay:YES];
    NSTimeInterval now = NSDate.timeIntervalSinceReferenceDate;
    if (!view.hidden && now - view->lastAccessibilityUpdate > 0.25) {
        NSAccessibilityPostNotification(view, NSAccessibilityValueChangedNotification);
        view->lastAccessibilityUpdate = now;
    }
}
void brig_terminal_focus(void *pointer) {
    BrigTerminalView *view = (__bridge BrigTerminalView *)pointer;
    [view.window makeFirstResponder:view];
}
void brig_terminal_show(void *pointer, bool visible) { ((__bridge BrigTerminalView *)pointer).hidden = !visible; }
void brig_terminal_destroy(void *pointer) {
    BrigTerminalView *view = (__bridge_transfer BrigTerminalView *)pointer;
    [view.inputContext discardMarkedText];
    [view removeFromSuperview];
}
void brig_terminal_copy_text(const uint8_t *bytes, size_t length) {
    NSString *text = [[NSString alloc] initWithBytes:bytes length:length encoding:NSUTF8StringEncoding];
    if (!text) return;
    [NSPasteboard.generalPasteboard clearContents];
    [NSPasteboard.generalPasteboard setString:text forType:NSPasteboardTypeString];
}
