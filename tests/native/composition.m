#import "TerminalViewPrivate.h"
#include <string.h>

static NSMutableData *captured;
static void receiveInput(uint64_t identifier, const uint8_t *bytes, size_t length) {
    NSCAssert(identifier == 17, @"Unexpected terminal identifier");
    [captured appendBytes:bytes length:length];
}

@interface BrigTerminalOverlayHost : NSView
@end
@implementation BrigTerminalOverlayHost
- (BOOL)isFlipped { return YES; }
@end

static NSColor *renderedPixel(BrigTerminalView *view, NSInteger x, NSInteger y) {
    NSBitmapImageRep *image = [[NSBitmapImageRep alloc] initWithBitmapDataPlanes:NULL pixelsWide:view.bounds.size.width pixelsHigh:view.bounds.size.height bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSDeviceRGBColorSpace bytesPerRow:0 bitsPerPixel:0];
    NSGraphicsContext *previous = NSGraphicsContext.currentContext;
    NSGraphicsContext.currentContext = [NSGraphicsContext graphicsContextWithBitmapImageRep:image];
    [[NSColor colorWithDeviceRed:1 green:0 blue:1 alpha:1] setFill];
    NSRectFill(view.bounds);
    [view drawRect:view.bounds];
    NSGraphicsContext.currentContext = previous;
    return [[image colorAtX:x y:y] colorUsingColorSpace:NSColorSpace.deviceRGBColorSpace];
}

static BOOL showsBackdrop(NSColor *color) {
    return color.redComponent > 0.99 && color.greenComponent < 0.01 && color.blueComponent > 0.99;
}

static void checkOverlay(void *pointer) {
    BrigTerminalView *view = (__bridge BrigTerminalView *)pointer;
    NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(-10000, -10000, 320, 240) styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
    window.releasedWhenClosed = NO;
    NSView *host = [[BrigTerminalOverlayHost alloc] initWithFrame:NSMakeRect(0, 0, 320, 240)];
    window.contentView = host;
    [host addSubview:view];
    brig_terminal_layout(pointer, 0, 0, 320, 240, true, 80, 60, 160, 120);
    NSCAssert(view.window == window && NSEqualRects(view.frame, NSMakeRect(0, 0, 320, 240)), @"Overlay layout requires a window-backed coordinate system");
    NSCAssert(NSEqualRects(view->overlayRectangle, NSMakeRect(80, 60, 160, 120)), @"Overlay bounds changed");
    NSCAssert(!view.isOpaque, @"An overlay must reveal the WebView below the terminal");
    NSCAssert([view hitTest:NSMakePoint(160, 120)] == nil, @"The overlay must not select hidden terminal text");
    NSCAssert([view hitTest:NSMakePoint(8, 8)] == view, @"Terminal input outside the overlay must remain available");
    NSCAssert(showsBackdrop(renderedPixel(view, 160, 120)), @"Terminal drawing covered the overlay");
    NSCAssert(!showsBackdrop(renderedPixel(view, 1, 1)), @"Terminal drawing outside the overlay disappeared");
    brig_terminal_layout(pointer, 0, 0, 320, 240, true, 300, 220, 80, 80);
    NSCAssert(NSEqualRects(view->overlayRectangle, NSMakeRect(300, 220, 20, 20)), @"Overlay bounds must be clipped to the terminal");
    brig_terminal_layout(pointer, 0, 0, 320, 240, true, 400, 300, 80, 80);
    NSCAssert(NSIsEmptyRect(view->overlayRectangle) && view.isOpaque, @"An outside overlay must not change terminal opacity");
    brig_terminal_layout(pointer, 0, 0, 320, 240, true, 80, 60, 160, 120);
    brig_terminal_layout(pointer, 0, 0, 320, 240, true, 0, 0, 0, 0);
    NSCAssert(view.isOpaque && NSIsEmptyRect(view->overlayRectangle), @"Clearing the overlay must restore the full terminal");
    NSCAssert([view hitTest:NSMakePoint(160, 120)] == view, @"Clearing the overlay must restore terminal input");
    NSCAssert(!showsBackdrop(renderedPixel(view, 160, 120)), @"Clearing the overlay must restore terminal drawing");
    [window close];
}

typedef struct {
    CGFloat delta;
    BOOL precise;
    NSEventModifierFlags modifiers;
    NSPoint position;
} BrigScrollInput;

@interface BrigTerminalScrollEvent : NSEvent {
@public
    BrigScrollInput scrollInput;
}
@end
@implementation BrigTerminalScrollEvent
- (NSEventType)type { return NSEventTypeScrollWheel; }
- (CGFloat)scrollingDeltaY { return scrollInput.delta; }
- (BOOL)hasPreciseScrollingDeltas { return scrollInput.precise; }
- (NSEventModifierFlags)modifierFlags { return scrollInput.modifiers; }
- (NSPoint)locationInWindow { return scrollInput.position; }
@end

@interface BrigTerminalUnscaledWindow : NSWindow
@end
@implementation BrigTerminalUnscaledWindow
- (CGFloat)backingScaleFactor { return 1; }
@end

static NSSize resizedScrollPixels;
static void receiveScrollResize(uint64_t identifier, uint16_t columns, uint16_t rows, uint16_t width, uint16_t height) {
    NSCAssert(identifier == 17 && columns > 0 && rows > 0, @"Unexpected terminal resize");
    resizedScrollPixels = NSMakeSize(width, height);
}

static void feedScrollTerminal(BrigTerminalView *view, const char *text) {
    brig_terminal_feed((__bridge void *)view, (const uint8_t *)text, strlen(text));
}

static void resetScrollTerminal(BrigTerminalView *view, const char *modes) {
    ghostty_terminal_reset(view->terminal);
    view->scrollRemainder = 0;
    feedScrollTerminal(view, modes);
    [captured setLength:0];
}

static void expectScrollAt(BrigTerminalView *view, CGFloat delta, BOOL precise, NSEventModifierFlags modifiers, NSPoint position, const char *expected) {
    BrigTerminalScrollEvent *event = [[BrigTerminalScrollEvent alloc] init];
    NSCAssert([event isKindOfClass:BrigTerminalScrollEvent.class], @"Could not initialize the scroll event fixture");
    event->scrollInput = (BrigScrollInput){delta, precise, modifiers, [view convertPoint:position toView:nil]};
    [captured setLength:0];
    [view scrollWheel:event];
    NSData *wanted = [NSData dataWithBytes:expected length:strlen(expected)];
    NSCAssert([captured isEqualToData:wanted], @"Wheel input changed: expected %@, received %@", wanted, captured);
}

static void expectScroll(BrigTerminalView *view, CGFloat delta, BOOL precise, NSEventModifierFlags modifiers, const char *expected) {
    NSPoint position = NSMakePoint(BrigTerminalPaddingX + view->cellWidth * 1.25, BrigTerminalPaddingY + view->cellHeight * 2.25);
    expectScrollAt(view, delta, precise, modifiers, position, expected);
}

static BOOL viewportActive(BrigTerminalView *view) {
    bool active = true;
    NSCAssert(ghostty_terminal_get(view->terminal, GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE, &active) == GHOSTTY_SUCCESS, @"Could not inspect scrollback position");
    return active;
}

static void checkMouseScroll(BrigTerminalView *view) {
    resetScrollTerminal(view, "\033[?1049h\033[?1000h\033[?1006h\033[?1l");
    expectScroll(view, 1, NO, 0, "\033[<64;2;3M");
    expectScroll(view, -1, NO, 0, "\033[<65;2;3M");
    expectScroll(view, 1, NO, NSEventModifierFlagShift | NSEventModifierFlagControl | NSEventModifierFlagOption, "\033[<92;2;3M");
    expectScroll(view, 0.1, NO, 0, "\033[<64;2;3M");
    expectScroll(view, view->cellHeight * 0.4, YES, 0, "");
    expectScroll(view, view->cellHeight * 0.4, YES, 0, "");
    expectScroll(view, view->cellHeight * 0.4, YES, 0, "\033[<64;2;3M");
    resetScrollTerminal(view, "\033[?1049h\033[?9h\033[?1007h");
    expectScroll(view, 1, NO, 0, "");
}

static void checkAlternateScroll(BrigTerminalView *view) {
    resetScrollTerminal(view, "\033[?1049h\033[?1007h\033[?1l");
    expectScroll(view, 1, NO, 0, "\033[A");
    expectScroll(view, -1, NO, 0, "\033[B");
    feedScrollTerminal(view, "\033[?1h");
    expectScroll(view, 1, NO, 0, "\033OA");
    expectScroll(view, -1, NO, 0, "\033OB");
    feedScrollTerminal(view, "\033[?1007l");
    expectScroll(view, 1, NO, 0, "");
    expectScroll(view, -1, NO, 0, "");
}

static void checkPrimaryScroll(void *pointer) {
    BrigTerminalView *view = (__bridge BrigTerminalView *)pointer;
    resetScrollTerminal(view, "");
    for (NSUInteger index = 0; index < 100; index++) feedScrollTerminal(view, "history row\r\n");
    NSCAssert(viewportActive(view), @"New output must start at the active viewport");
    expectScroll(view, 1, NO, 0, "");
    NSCAssert(!viewportActive(view), @"Primary-screen wheel input did not reach scrollback");
    GhosttyTerminalScrollbar before = {0}, after = {0};
    ghostty_terminal_get(view->terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &before);
    feedScrollTerminal(view, "new output\r\n");
    brig_terminal_focus(pointer);
    ghostty_terminal_get(view->terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &after);
    NSCAssert(!viewportActive(view) && before.offset == after.offset, @"Output or focus moved the scrollback viewport");
    feedScrollTerminal(view, "\033[?1000h\033[?1006h");
    expectScroll(view, 1, NO, 0, "\033[<64;2;3M");
    expectScroll(view, -1, NO, 0, "\033[<65;2;3M");
    NSCAssert(!viewportActive(view), @"Mouse reports must not reset the native viewport");
    [captured setLength:0];
    [view insertText:@"x" replacementRange:NSMakeRange(NSNotFound, 0)];
    NSCAssert(viewportActive(view) && [captured isEqualToData:[@"x" dataUsingEncoding:NSUTF8StringEncoding]], @"Keyboard input must return to the active viewport");
}

static NSEvent *selectionEvent(BrigTerminalView *view, NSEventType type, NSUInteger column, NSUInteger row, NSEventModifierFlags flags) {
    NSPoint cell = NSMakePoint(BrigTerminalPaddingX + (column + 0.5) * view->cellWidth,
                               BrigTerminalPaddingY + (row + 0.5) * view->cellHeight);
    return [NSEvent mouseEventWithType:type location:[view convertPoint:cell toView:nil] modifierFlags:flags timestamp:0 windowNumber:view.window.windowNumber context:nil eventNumber:0 clickCount:1 pressure:0];
}

static void checkSelection(BrigTerminalView *view) {
    resetScrollTerminal(view, "LEFT1   AAA   RIGHT1\r\nLEFT2   BBB   RIGHT2\r\nLEFT3   CCC   RIGHT3");
    NSString *rectangle = @"AAA\nBBB\nCCC";
    [view mouseDown:selectionEvent(view, NSEventTypeLeftMouseDown, 8, 0, NSEventModifierFlagOption)];
    [view mouseDragged:selectionEvent(view, NSEventTypeLeftMouseDragged, 10, 2, 0)];
    [view->dragTimer fire];
    [view mouseUp:selectionEvent(view, NSEventTypeLeftMouseUp, 10, 2, 0)];
    NSCAssert([[view formattedText:YES] isEqualToString:rectangle], @"Option at mouse down must preserve only the selected columns through the timer and mouse up");

    [view mouseDown:selectionEvent(view, NSEventTypeLeftMouseDown, 10, 2, NSEventModifierFlagOption)];
    [view mouseDragged:selectionEvent(view, NSEventTypeLeftMouseDragged, 8, 0, NSEventModifierFlagOption)];
    [view mouseUp:selectionEvent(view, NSEventTypeLeftMouseUp, 8, 0, 0)];
    NSCAssert([[view formattedText:YES] isEqualToString:rectangle], @"A reverse Option-drag must copy the same rectangle");

    [view mouseDown:selectionEvent(view, NSEventTypeLeftMouseDown, 8, 0, 0)];
    [view mouseDragged:selectionEvent(view, NSEventTypeLeftMouseDragged, 10, 2, 0)];
    [view mouseUp:selectionEvent(view, NSEventTypeLeftMouseUp, 10, 2, 0)];
    NSString *linear = [view formattedText:YES];
    NSCAssert([linear hasPrefix:@"AAA"] && [linear containsString:@"LEFT2   BBB   RIGHT2"] && [linear hasSuffix:@"CCC"], @"The next plain drag must select a continuous text range");
}

static void checkTerminalGestures(void *pointer) {
    BrigTerminalView *view = (__bridge BrigTerminalView *)pointer;
    NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(-10000, -10000, 320, 240) styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
    window.releasedWhenClosed = NO;
    window.contentView = [[BrigTerminalOverlayHost alloc] initWithFrame:NSMakeRect(0, 0, 320, 240)];
    [window.contentView addSubview:view];
    brig_terminal_layout(pointer, 0, 0, 320, 240, true, 0, 0, 0, 0);
    checkSelection(view);
    checkMouseScroll(view);
    checkAlternateScroll(view);
    checkPrimaryScroll(pointer);
    [window close];
}

static void checkHalfPointScroll(void *pointer) {
    BrigTerminalView *view = (__bridge BrigTerminalView *)pointer;
    CGFloat originalWidth = view->cellWidth;
    NSSize originalSize = view.frame.size;
    BrigTerminalResize originalResize = view->resize;
    NSWindow *window = [[BrigTerminalUnscaledWindow alloc] initWithContentRect:NSMakeRect(-10000, -10000, 640, 240) styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
    window.releasedWhenClosed = NO;
    window.contentView = [[BrigTerminalOverlayHost alloc] initWithFrame:NSMakeRect(0, 0, 640, 240)];
    [window.contentView addSubview:view];
    view->cellWidth = 8.5;
    view->resize = receiveScrollResize;
    resizedScrollPixels = NSZeroSize;
    brig_terminal_layout(pointer, 0, 0, 640, 240, true, 0, 0, 0, 0);
    [view updateGrid];
    NSCAssert(window.backingScaleFactor == 1 && view->pixelWidth == view->columns * 17 && view->pixelHeight == view->rows * view->cellHeight * 2, @"Half-point cells must retain integer virtual pixel geometry on a 1x display");
    NSCAssert(resizedScrollPixels.width == view->pixelWidth && resizedScrollPixels.height == view->pixelHeight, @"The PTY must receive the same virtual pixel geometry");
    uint32_t engineWidth = 0, engineHeight = 0;
    ghostty_terminal_get(view->terminal, GHOSTTY_TERMINAL_DATA_WIDTH_PX, &engineWidth);
    ghostty_terminal_get(view->terminal, GHOSTTY_TERMINAL_DATA_HEIGHT_PX, &engineHeight);
    NSCAssert(engineWidth == view->pixelWidth && engineHeight == view->pixelHeight, @"Ghostty and PTY pixel sizes must agree");
    resetScrollTerminal(view, "\033[?1000h\033[?1006h");
    NSPoint position = NSMakePoint(BrigTerminalPaddingX + view->cellWidth * 30.25, BrigTerminalPaddingY + view->cellHeight * 2.25);
    expectScrollAt(view, 1, NO, 0, position, "\033[<64;31;3M");
    feedScrollTerminal(view, "\033[?1016h");
    NSString *pixels = [NSString stringWithFormat:@"\033[<64;514;%ldM", (long)round(view->cellHeight * 2.25 * 2)];
    expectScrollAt(view, 1, NO, 0, position, pixels.UTF8String);
    view->cellWidth = originalWidth;
    view->resize = originalResize;
    [view setFrameSize:originalSize];
    [view updateGrid];
    [window close];
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        NSCAssert(argc == 2, @"Pass the bundled JetBrains Mono font path");
        NSURL *fallbackURL = [NSURL fileURLWithPath:[NSString stringWithUTF8String:argv[1]]];
        NSFont *fallback = brigTerminalFont(@"BrigMissingFontForFallbackTest", fallbackURL);
        NSCAssert([fallback.familyName isEqualToString:@"JetBrains Mono"] && fallback.pointSize == 14, @"The bundled 14pt fallback did not load");
        CGFloat narrow = [@"iiii" sizeWithAttributes:@{NSFontAttributeName:fallback}].width;
        CGFloat wide = [@"WWWW" sizeWithAttributes:@{NSFontAttributeName:fallback}].width;
        NSCAssert(fabs(narrow - wide) < 0.01, @"The fallback must preserve terminal columns");
        NSFont *preferred = brigTerminalFont(@"Menlo", fallbackURL);
        NSCAssert([preferred.familyName isEqualToString:@"Menlo"] && preferred.pointSize == 14, @"An available preferred font must win");
        captured = [NSMutableData data];
        void *pointer = brig_terminal_create(NULL, 17, receiveInput, NULL);
        NSCAssert(pointer, @"Could not initialize the native terminal");
        BrigTerminalView *view = (__bridge BrigTerminalView *)pointer;
        NSCAssert(view->font.pointSize == 14, @"The terminal font size changed");
        NSEvent *drag = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDragged location:NSMakePoint(30, 30) modifierFlags:0 timestamp:0 windowNumber:0 context:nil eventNumber:0 clickCount:1 pressure:0];
        [view mouseDragged:drag];
        NSCAssert(view->dragTimer == nil, @"Dragging without a selection anchor must not start a timer");
        [view mouseDown:drag];
        NSCAssert(view->selectionState == BrigSelectionAnchored, @"Mouse down must establish an anchor");
        [view mouseDragged:drag];
        NSCAssert(view->selectionState == BrigSelectionDragging && view->dragTimer, @"Anchored drag must start its timer");
        view.hidden = YES;
        [view->dragTimer fire];
        NSCAssert(view->selectionState == BrigSelectionIdle && !view->dragTimer && !view->selectionAnchor, @"Hidden drag must release its timer and anchor");
        view.hidden = NO;
        [view mouseUp:drag];
        [view setMarkedText:@"u" selectedRange:NSMakeRange(1, 0) replacementRange:NSMakeRange(NSNotFound, 0)];
        NSCAssert(view.hasMarkedText, @"Composition must remain marked before commit");
        NSCAssert(view.markedRange.length == 1 && view.selectedRange.location == 1, @"Incorrect marked selection");
        NSCAssert(captured.length == 0, @"Uncommitted text reached the PTY");
        NSString *text = @"ü日本🙂";
        [view insertText:text replacementRange:NSMakeRange(NSNotFound, 0)];
        NSCAssert(!view.hasMarkedText, @"Committed composition was not cleared");
        NSCAssert([captured isEqualToData:[text dataUsingEncoding:NSUTF8StringEncoding]], @"The UTF-8 commit changed");
        [captured setLength:0];
        NSEvent *escape = [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil characters:@"\033" charactersIgnoringModifiers:@"\033" isARepeat:NO keyCode:53];
        [view setMarkedText:@"^" selectedRange:NSMakeRange(1, 0) replacementRange:NSMakeRange(NSNotFound, 0)];
        [view keyDown:escape];
        NSCAssert(!view.hasMarkedText && captured.length == 0, @"Escape must discard marked text without sending input");
        [view keyDown:escape];
        NSCAssert([captured isEqualToData:[NSData dataWithBytes:"\033" length:1]], @"Escape outside composition must reach the PTY");
        [view setMarkedText:@"" selectedRange:NSMakeRange(0, 0) replacementRange:NSMakeRange(NSNotFound, 0)];
        NSCAssert(view->compositionState == BrigCompositionIdle && view.selectedRange.location == NSNotFound, @"Empty composition must be idle");
        [view unmarkText];
        NSCAssert(view->compositionState == BrigCompositionIdle, @"Repeated cancellation must remain idle");
        [captured setLength:0];
        NSEvent *event = [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil characters:@"" charactersIgnoringModifiers:@"" isARepeat:NO keyCode:120];
        [view keyDown:event];
        NSCAssert([captured isEqualToData:[NSData dataWithBytes:"\033OQ" length:3]], @"F2 did not produce the xterm key sequence");
        NSArray<NSString *> *controlKeys = @[@"2", @"/", @"c"];
        const unsigned short codes[] = {19, 44, 8};
        const uint8_t expected[] = {0x00, 0x1f, 0x03};
        for (NSUInteger i = 0; i < controlKeys.count; i++) {
            [captured setLength:0];
            NSEvent *control = [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:NSEventModifierFlagControl timestamp:0 windowNumber:0 context:nil characters:controlKeys[i] charactersIgnoringModifiers:controlKeys[i] isARepeat:NO keyCode:codes[i]];
            [view keyDown:control];
            NSCAssert([captured isEqualToData:[NSData dataWithBytes:&expected[i] length:1]], @"Unexpected bytes for Ctrl+%@", controlKeys[i]);
        }
        checkOverlay(pointer);
        checkTerminalGestures(pointer);
        checkHalfPointScroll(pointer);
        brig_terminal_destroy(pointer);
        puts("PASS native fonts: preferred font, bundled JetBrains Mono fallback, monospace metrics and 14pt size");
        puts("PASS native composition: marked text, UTF-8 commit, cancellation and F2 encoding");
        puts("PASS native overlay: clipped drawing, bounded exclusion, hit testing and full restoration");
        puts("PASS native selection: forward and reverse Option-drag, timer updates and plain drag");
        puts("PASS native scrolling: TUI mouse reports, cursor modes, wheel deltas and retained primary scrollback");
        puts("PASS native mouse geometry: half-point cells, far columns and SGR-Pixels on a 1x display");
    }
    return 0;
}
