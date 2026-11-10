#include "TerminalViewPrivate.h"

static bool readPaste(void *context, GhosttyString mime, GhosttyWriter writer) {
    (void)mime;
    NSData *data = (__bridge NSData *)context;
    return !data.length || writer.write(writer.userdata, data.bytes, data.length);
}

@implementation BrigTerminalView (Input)
- (void)sendData:(NSData *)data {
    if (!input || !data.length) return;
    ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
    GhosttyTerminalScrollViewport bottom = {.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM};
    ghostty_terminal_scroll_viewport(terminal, bottom);
    input(identifier, data.bytes, data.length);
    [self setNeedsDisplay:YES];
}
- (GhosttyPoint)pointForPosition:(NSPoint)position {
    uint16_t column = MAX(0, MIN(columns - 1, floor((position.x - BrigTerminalPaddingX) / cellWidth)));
    uint32_t row = MAX(0, MIN(rows - 1, floor((position.y - BrigTerminalPaddingY) / cellHeight)));
    return (GhosttyPoint){.tag = GHOSTTY_POINT_TAG_VIEWPORT, .value = {.coordinate = {.x = column, .y = row}}};
}
- (void)mouseDown:(NSEvent *)event {
    [self.window makeFirstResponder:self];
    ghostty_tracked_grid_ref_free(selectionAnchor); selectionAnchor = NULL;
    GhosttyPoint point = [self pointForPosition:[self convertPoint:event.locationInWindow fromView:nil]];
    ghostty_terminal_grid_ref_track(terminal, point, &selectionAnchor);
    ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
    if (event.clickCount == 2) {
        GhosttyTerminalSelectWordOptions options = GHOSTTY_INIT_SIZED(GhosttyTerminalSelectWordOptions);
        GhosttySelection selection = {0};
        if (ghostty_terminal_grid_ref(terminal, point, &options.ref) == GHOSTTY_SUCCESS && ghostty_terminal_select_word(terminal, &options, &selection) == GHOSTTY_SUCCESS) ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &selection);
    }
    [self setNeedsDisplay:YES];
}
- (void)updateDraggedSelection {
    if (!selectionAnchor) return;
    int delta = dragPoint.y < BrigTerminalPaddingY ? -1 : (dragPoint.y > NSMaxY(self.bounds) - BrigTerminalPaddingY ? 1 : 0);
    if (delta) { GhosttyTerminalScrollViewport scroll = {.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA, .value = {.delta = delta}}; ghostty_terminal_scroll_viewport(terminal, scroll); }
    GhosttySelection selection = {0};
    if (ghostty_tracked_grid_ref_snapshot(selectionAnchor, &selection.start) != GHOSTTY_SUCCESS) return;
    if (ghostty_terminal_grid_ref(terminal, [self pointForPosition:dragPoint], &selection.end) != GHOSTTY_SUCCESS) return;
    selection.rectangle = NSEvent.modifierFlags & NSEventModifierFlagOption;
    ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &selection);
    [self setNeedsDisplay:YES];
}
- (void)mouseDragged:(NSEvent *)event {
    dragPoint = [self convertPoint:event.locationInWindow fromView:nil];
    [self updateDraggedSelection];
    if (dragTimer) return;
    __weak BrigTerminalView *weakView = self;
    dragTimer = [NSTimer scheduledTimerWithTimeInterval:0.08 repeats:YES block:^(NSTimer *timer) {
        BrigTerminalView *view = weakView;
        if (!view || view.hidden) { [timer invalidate]; return; }
        [view updateDraggedSelection];
    }];
}
- (void)mouseUp:(NSEvent *)event {
    (void)event;
    [dragTimer invalidate]; dragTimer = nil;
    ghostty_tracked_grid_ref_free(selectionAnchor); selectionAnchor = NULL;
}
- (void)scrollWheel:(NSEvent *)event {
    scrollRemainder += event.hasPreciseScrollingDeltas ? -event.scrollingDeltaY / cellHeight : -event.scrollingDeltaY;
    int delta = (int)scrollRemainder;
    scrollRemainder -= delta;
    if (!delta) return;
    GhosttyTerminalScreen screen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
    ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen);
    if (screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE) {
        for (int index = 0; index < MIN(12, abs(delta)); index++) [self encodeKey:delta < 0 ? GHOSTTY_KEY_ARROW_UP : GHOSTTY_KEY_ARROW_DOWN modifiers:0 repeating:NO];
        return;
    }
    GhosttyTerminalScrollViewport scroll = {.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA, .value = {.delta = delta}};
    ghostty_terminal_scroll_viewport(terminal, scroll);
    [self setNeedsDisplay:YES];
}
- (void)encodeKey:(GhosttyKey)key modifiers:(GhosttyMods)modifiers repeating:(BOOL)repeat {
    ghostty_key_encoder_set_from_terminal(keyEncoder, terminal);
    ghostty_key_event_set_action(keyEvent, repeat ? GHOSTTY_KEY_ACTION_REPEAT : GHOSTTY_KEY_ACTION_PRESS);
    ghostty_key_event_set_key(keyEvent, key);
    ghostty_key_event_set_mods(keyEvent, modifiers);
    ghostty_key_event_set_utf8(keyEvent, (GhosttyString){0});
    uint8_t bytes[128]; size_t length = 0;
    if (ghostty_key_encoder_encode(keyEncoder, keyEvent, bytes, sizeof(bytes), &length) == GHOSTTY_SUCCESS) [self sendData:[NSData dataWithBytes:bytes length:length]];
}
- (void)keyDown:(NSEvent *)event {
    if (event.modifierFlags & NSEventModifierFlagCommand) { [super keyDown:event]; return; }
    if (self.hasMarkedText) { [self interpretKeyEvents:@[event]]; return; }
    GhosttyKey key = GHOSTTY_KEY_UNIDENTIFIED;
    switch (event.keyCode) {
        case 123: key = GHOSTTY_KEY_ARROW_LEFT; break;
        case 124: key = GHOSTTY_KEY_ARROW_RIGHT; break;
        case 125: key = GHOSTTY_KEY_ARROW_DOWN; break;
        case 126: key = GHOSTTY_KEY_ARROW_UP; break;
        case 36: case 76: key = GHOSTTY_KEY_ENTER; break;
        case 53: key = GHOSTTY_KEY_ESCAPE; break;
        case 51: key = GHOSTTY_KEY_BACKSPACE; break;
        case 48: key = GHOSTTY_KEY_TAB; break;
        case 115: key = GHOSTTY_KEY_HOME; break;
        case 119: key = GHOSTTY_KEY_END; break;
        case 116: key = GHOSTTY_KEY_PAGE_UP; break;
        case 121: key = GHOSTTY_KEY_PAGE_DOWN; break;
        case 117: key = GHOSTTY_KEY_DELETE; break;
        case 122: key = GHOSTTY_KEY_F1; break;
        case 120: key = GHOSTTY_KEY_F2; break;
        case 99: key = GHOSTTY_KEY_F3; break;
        case 118: key = GHOSTTY_KEY_F4; break;
        case 96: key = GHOSTTY_KEY_F5; break;
        case 97: key = GHOSTTY_KEY_F6; break;
        case 98: key = GHOSTTY_KEY_F7; break;
        case 100: key = GHOSTTY_KEY_F8; break;
        case 101: key = GHOSTTY_KEY_F9; break;
        case 109: key = GHOSTTY_KEY_F10; break;
        case 103: key = GHOSTTY_KEY_F11; break;
        case 111: key = GHOSTTY_KEY_F12; break;
        case 105: key = GHOSTTY_KEY_F13; break;
        case 107: key = GHOSTTY_KEY_F14; break;
        case 113: key = GHOSTTY_KEY_F15; break;
        case 106: key = GHOSTTY_KEY_F16; break;
        case 64: key = GHOSTTY_KEY_F17; break;
        case 79: key = GHOSTTY_KEY_F18; break;
        case 80: key = GHOSTTY_KEY_F19; break;
        case 90: key = GHOSTTY_KEY_F20; break;
        default: break;
    }
    if (key == GHOSTTY_KEY_UNIDENTIFIED) {
        if (event.modifierFlags & NSEventModifierFlagControl) {
            NSString *characters = event.charactersIgnoringModifiers;
            if (characters.length) { uint8_t byte = [characters characterAtIndex:0] & 0x1f; [self sendData:[NSData dataWithBytes:&byte length:1]]; }
            return;
        }
        [self interpretKeyEvents:@[event]]; return;
    }
    GhosttyMods modifiers = 0;
    if (event.modifierFlags & NSEventModifierFlagShift) modifiers |= GHOSTTY_MODS_SHIFT;
    if (event.modifierFlags & NSEventModifierFlagControl) modifiers |= GHOSTTY_MODS_CTRL;
    if (event.modifierFlags & NSEventModifierFlagOption) modifiers |= GHOSTTY_MODS_ALT;
    [self encodeKey:key modifiers:modifiers repeating:event.isARepeat];
}
- (void)copy:(id)sender {
    (void)sender;
    NSString *text = [self formattedText:YES];
    if (!text) { NSBeep(); return; }
    [NSPasteboard.generalPasteboard clearContents];
    [NSPasteboard.generalPasteboard setString:text forType:NSPasteboardTypeString];
}
- (void)paste:(id)sender {
    (void)sender;
    NSString *text = [NSPasteboard.generalPasteboard stringForType:NSPasteboardTypeString];
    NSData *data = [text dataUsingEncoding:NSUTF8StringEncoding];
    if (!data.length) return;
    if (data.length > 1024 * 1024) { NSBeep(); return; }
    GhosttyString mime = {.ptr = (const uint8_t *)"text/plain", .len = 10};
    GhosttyPaste paste = GHOSTTY_INIT_SIZED(GhosttyPaste);
    paste.location = GHOSTTY_CLIPBOARD_LOCATION_STANDARD; paste.source = GHOSTTY_PASTE_SOURCE_CLIPBOARD;
    paste.mimes = &mime; paste.mimes_len = 1; paste.allow_unsafe = true;
    paste.reader = (GhosttyMimeReader){.read = readPaste, .userdata = (__bridge void *)data};
    if (ghostty_terminal_paste(terminal, &paste, NULL) != GHOSTTY_SUCCESS) NSBeep();
    [self setNeedsDisplay:YES];
}
- (void)selectAll:(id)sender {
    (void)sender;
    GhosttySelection selection = {0};
    if (ghostty_terminal_select_all(terminal, &selection) == GHOSTTY_SUCCESS) ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &selection);
    [self setNeedsDisplay:YES];
}
- (BOOL)validateUserInterfaceItem:(id<NSValidatedUserInterfaceItem>)item {
    if (item.action == @selector(copy:)) { GhosttySelection selection; return ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_SELECTION, &selection) == GHOSTTY_SUCCESS; }
    if (item.action == @selector(paste:)) return [NSPasteboard.generalPasteboard availableTypeFromArray:@[NSPasteboardTypeString]] != nil;
    return item.action == @selector(selectAll:);
}
- (void)insertText:(id)value replacementRange:(NSRange)range {
    (void)range;
    NSString *string = [value isKindOfClass:NSAttributedString.class] ? [value string] : value;
    [markedText setAttributedString:[[NSAttributedString alloc] initWithString:@""]];
    [self sendData:[string dataUsingEncoding:NSUTF8StringEncoding]];
}
- (void)setMarkedText:(id)value selectedRange:(NSRange)selection replacementRange:(NSRange)range {
    (void)range;
    NSAttributedString *text = [value isKindOfClass:NSAttributedString.class] ? value : [[NSAttributedString alloc] initWithString:value];
    [markedText setAttributedString:text]; markedSelection = selection;
    [self setNeedsDisplay:YES];
}
- (void)unmarkText { [markedText setAttributedString:[[NSAttributedString alloc] initWithString:@""]]; [self setNeedsDisplay:YES]; }
- (BOOL)hasMarkedText { return markedText.length > 0; }
- (NSRange)markedRange { return self.hasMarkedText ? NSMakeRange(0, markedText.length) : NSMakeRange(NSNotFound, 0); }
- (NSRange)selectedRange { return self.hasMarkedText ? markedSelection : NSMakeRange(NSNotFound, 0); }
- (NSArray *)validAttributesForMarkedText { return @[NSUnderlineStyleAttributeName, NSMarkedClauseSegmentAttributeName]; }
- (NSAttributedString *)attributedSubstringForProposedRange:(NSRange)range actualRange:(NSRangePointer)actual {
    if (!markedText.length || range.location == NSNotFound) return nil;
    NSRange intersection = NSIntersectionRange(range, NSMakeRange(0, markedText.length));
    if (actual) *actual = intersection;
    return [markedText attributedSubstringFromRange:intersection];
}
- (NSUInteger)characterIndexForPoint:(NSPoint)point { (void)point; return NSNotFound; }
- (NSRect)firstRectForCharacterRange:(NSRange)range actualRange:(NSRangePointer)actual {
    (void)range;
    if (actual) *actual = self.markedRange;
    NSRect rectangle = [self convertRect:self.cursorRectangle toView:nil];
    return [self.window convertRectToScreen:rectangle];
}
- (void)doCommandBySelector:(SEL)selector {
    if (selector == @selector(cancelOperation:)) [self encodeKey:GHOSTTY_KEY_ESCAPE modifiers:0 repeating:NO];
    if (selector == @selector(insertNewline:)) [self encodeKey:GHOSTTY_KEY_ENTER modifiers:0 repeating:NO];
}
@end
