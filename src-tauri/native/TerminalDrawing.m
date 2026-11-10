#include "TerminalViewPrivate.h"

static NSColor *terminalColor(GhosttyColorRgb value) {
    return [NSColor colorWithSRGBRed:value.r / 255.0 green:value.g / 255.0 blue:value.b / 255.0 alpha:1];
}
static void cellColors(GhosttyRenderStateRowCells cells, GhosttyRenderStateColors defaults, NSColor **background, NSColor **foreground, GhosttyStyle *style) {
    GhosttyColorRgb bg = defaults.background, fg = defaults.foreground;
    ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &bg);
    ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &fg);
    ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, style);
    if (style->flags & GHOSTTY_STYLE_FLAG_INVERSE) { GhosttyColorRgb swap = fg; fg = bg; bg = swap; }
    bool selected = false;
    ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED, &selected);
    *background = selected ? terminalColor(brigTerminalRGB(BrigTerminalSelectionBackground)) : terminalColor(bg);
    *foreground = selected ? terminalColor(brigTerminalRGB(BrigTerminalSelectionForeground)) : terminalColor(fg);
    if (style->flags & GHOSTTY_STYLE_FLAG_FAINT) *foreground = [*foreground colorWithAlphaComponent:0.65];
}
static NSString *cellText(GhosttyRenderStateRowCells cells) {
    uint8_t text[256];
    GhosttyBuffer buffer = {.ptr = text, .cap = sizeof(text)};
    GhosttyResult result = ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, &buffer);
    if (result == GHOSTTY_OUT_OF_SPACE && buffer.len <= 4096) {
        NSMutableData *expanded = [NSMutableData dataWithLength:buffer.len];
        buffer.ptr = expanded.mutableBytes; buffer.cap = expanded.length;
        result = ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, &buffer);
        if (result == GHOSTTY_SUCCESS) return [[NSString alloc] initWithData:expanded encoding:NSUTF8StringEncoding];
    }
    if (result != GHOSTTY_SUCCESS || !buffer.len) return nil;
    return [[NSString alloc] initWithBytes:buffer.ptr length:buffer.len encoding:NSUTF8StringEncoding];
}
static bool appendFormatted(void *context, const uint8_t *bytes, size_t length) {
    NSMutableData *data = (__bridge NSMutableData *)context;
    if (length > 2 * 1024 * 1024 - data.length) return false;
    [data appendBytes:bytes length:length];
    return true;
}

@implementation BrigTerminalView (Drawing)
- (NSRect)cursorRectangle {
    GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
    ghostty_render_state_get(renderState, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor);
    if (!cursor.viewport_has_value) return NSMakeRect(BrigTerminalPaddingX, BrigTerminalPaddingY, cellWidth, cellHeight);
    return NSMakeRect(BrigTerminalPaddingX + cursor.viewport_x * cellWidth, BrigTerminalPaddingY + cursor.viewport_y * cellHeight, cellWidth, cellHeight);
}
- (void)drawCellText:(NSString *)text at:(NSPoint)point color:(NSColor *)color style:(GhosttyStyle)style {
    if (!text.length || (style.flags & GHOSTTY_STYLE_FLAG_INVISIBLE)) return;
    NSString *key = [NSString stringWithFormat:@"%hu/%@/%@", style.flags, color, text];
    id cached = [glyphCache objectForKey:key];
    if (!cached) {
        NSFontTraitMask traits = 0;
        if (style.flags & GHOSTTY_STYLE_FLAG_BOLD) traits |= NSBoldFontMask;
        if (style.flags & GHOSTTY_STYLE_FLAG_ITALIC) traits |= NSItalicFontMask;
        NSFont *currentFont = traits ? [[NSFontManager sharedFontManager] convertFont:font toHaveTrait:traits] : font;
        NSMutableDictionary *attributes = [@{(__bridge NSString *)kCTFontAttributeName:currentFont, (__bridge NSString *)kCTForegroundColorAttributeName:(__bridge id)color.CGColor} mutableCopy];
        if (GHOSTTY_STYLE_UNDERLINE(style) != GHOSTTY_SGR_UNDERLINE_NONE) attributes[(__bridge NSString *)kCTUnderlineStyleAttributeName] = @(kCTUnderlineStyleSingle);
        NSAttributedString *string = [[NSAttributedString alloc] initWithString:text attributes:attributes];
        cached = CFBridgingRelease(CTLineCreateWithAttributedString((__bridge CFAttributedStringRef)string));
        [glyphCache setObject:cached forKey:key cost:text.length * 2 + 256];
    }
    CGContextRef context = NSGraphicsContext.currentContext.CGContext;
    CGContextSaveGState(context);
    CGContextTranslateCTM(context, point.x, point.y + (cellHeight - font.ascender + font.descender) / 2 + font.ascender);
    CGContextScaleCTM(context, 1, -1);
    CGContextSetTextMatrix(context, CGAffineTransformIdentity);
    CGContextSetTextPosition(context, 0, 0);
    CTLineDraw((__bridge CTLineRef)cached, context);
    CGContextRestoreGState(context);
    if (style.flags & GHOSTTY_STYLE_FLAG_STRIKETHROUGH) { [color setFill]; NSRectFill(NSMakeRect(point.x, point.y + cellHeight / 2, cellWidth, 1)); }
}
- (void)drawRect:(NSRect)dirtyRect {
    if (ghostty_render_state_update(renderState, terminal) != GHOSTTY_SUCCESS) return;
    GhosttyRenderStateColors colors = GHOSTTY_INIT_SIZED(GhosttyRenderStateColors);
    ghostty_render_state_get(renderState, GHOSTTY_RENDER_STATE_DATA_COLORS, &colors);
    GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
    ghostty_render_state_get(renderState, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor);
    NSString *cursorText = nil;
    GhosttyStyle cursorStyle = {0};
    [terminalColor(colors.background) setFill]; NSRectFill(self.bounds);
    ghostty_render_state_get(renderState, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &rowIterator);
    NSUInteger row = 0;
    while (ghostty_render_state_row_iterator_next(rowIterator)) {
        // Backgrounds precede glyphs so a wide grapheme can span its following cell.
        for (NSUInteger pass = 0; pass < 2; pass++) {
            ghostty_render_state_row_get(rowIterator, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &cells);
            NSUInteger column = 0;
            while (ghostty_render_state_row_cells_next(cells)) {
                NSColor *background, *foreground; GhosttyStyle style = {0};
                cellColors(cells, colors, &background, &foreground, &style);
                NSPoint point = NSMakePoint(BrigTerminalPaddingX + column * cellWidth, BrigTerminalPaddingY + row * cellHeight);
                if (pass == 0) { [background setFill]; NSRectFill(NSMakeRect(point.x, point.y, cellWidth, cellHeight)); }
                if (pass == 1) {
                    NSString *text = cellText(cells);
                    [self drawCellText:text at:point color:foreground style:style];
                    if (cursor.viewport_has_value && row == cursor.viewport_y && column == cursor.viewport_x) { cursorText = text; cursorStyle = style; }
                }
                column++;
            }
        }
        row++;
    }
    BOOL active = self.window.isKeyWindow && self.window.firstResponder == self;
    BOOL blinkOff = active && cursor.blinking && !NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceMotion && fmod(NSDate.timeIntervalSinceReferenceDate, 1.0) > 0.5;
    if (cursor.visible && cursor.viewport_has_value && !blinkOff) {
        NSRect rectangle = self.cursorRectangle;
        NSColor *color = terminalColor(colors.cursor_has_value ? colors.cursor : colors.foreground);
        [color setFill]; [color setStroke];
        if (!active || cursor.visual_style == GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW) NSFrameRectWithWidth(rectangle, 1);
        if (active && cursor.visual_style == GHOSTTY_CURSOR_STYLE_BAR) NSRectFill(NSMakeRect(rectangle.origin.x, rectangle.origin.y, 2, rectangle.size.height));
        if (active && cursor.visual_style == GHOSTTY_CURSOR_STYLE_UNDERLINE) NSRectFill(NSMakeRect(rectangle.origin.x, NSMaxY(rectangle) - 2, rectangle.size.width, 2));
        if (active && cursor.visual_style == GHOSTTY_CURSOR_STYLE_BLOCK) {
            NSRectFill(rectangle);
            [self drawCellText:cursorText at:rectangle.origin color:terminalColor(colors.background) style:cursorStyle];
        }
    }
    if (markedText.length) {
        NSRect rectangle = self.cursorRectangle;
        [terminalColor(colors.background) setFill]; NSRectFill(NSMakeRect(rectangle.origin.x, rectangle.origin.y, MAX(cellWidth, markedText.size.width), cellHeight));
        [markedText.string drawAtPoint:rectangle.origin withAttributes:@{NSFontAttributeName:font, NSForegroundColorAttributeName:terminalColor(colors.foreground), NSUnderlineStyleAttributeName:@(NSUnderlineStyleSingle)}];
    }
}
- (NSString *)formattedText:(BOOL)selectionOnly {
    GhosttySelection selection = {0};
    if (selectionOnly && ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_SELECTION, &selection) != GHOSTTY_SUCCESS) return nil;
    if (!selectionOnly) {
        GhosttyPoint start = {.tag = GHOSTTY_POINT_TAG_VIEWPORT, .value = {.coordinate = {.x = 0, .y = 0}}};
        GhosttyPoint end = {.tag = GHOSTTY_POINT_TAG_VIEWPORT, .value = {.coordinate = {.x = columns - 1, .y = rows - 1}}};
        if (ghostty_terminal_grid_ref(terminal, start, &selection.start) != GHOSTTY_SUCCESS || ghostty_terminal_grid_ref(terminal, end, &selection.end) != GHOSTTY_SUCCESS) return @"";
    }
    ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_SELECTION, &selection);
    bool unwrap = selectionOnly;
    ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_UNWRAP, &unwrap);
    NSMutableData *data = [NSMutableData data];
    GhosttyWriter writer = {.write = appendFormatted, .userdata = (__bridge void *)data};
    if (ghostty_formatter_format(formatter, writer) != GHOSTTY_SUCCESS) return nil;
    return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}
- (id)accessibilityValue { return [self formattedText:NO] ?: @""; }
- (NSString *)accessibilitySelectedText { return [self formattedText:YES] ?: @""; }
- (NSNumber *)accessibilityNumberOfCharacters { return @([[self formattedText:NO] length]); }
@end
