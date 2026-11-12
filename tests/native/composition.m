#import "TerminalViewPrivate.h"

static NSMutableData *captured;
static void receiveInput(uint64_t identifier, const uint8_t *bytes, size_t length) {
    NSCAssert(identifier == 17, @"Unexpected terminal identifier");
    [captured appendBytes:bytes length:length];
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
        [view setMarkedText:@"u" selectedRange:NSMakeRange(1, 0) replacementRange:NSMakeRange(NSNotFound, 0)];
        NSCAssert(view.hasMarkedText, @"Composition must remain marked before commit");
        NSCAssert(view.markedRange.length == 1 && view.selectedRange.location == 1, @"Incorrect marked selection");
        NSCAssert(captured.length == 0, @"Uncommitted text reached the PTY");
        NSString *text = @"ü日本🙂";
        [view insertText:text replacementRange:NSMakeRange(NSNotFound, 0)];
        NSCAssert(!view.hasMarkedText, @"Committed composition was not cleared");
        NSCAssert([captured isEqualToData:[text dataUsingEncoding:NSUTF8StringEncoding]], @"The UTF-8 commit changed");
        [view setMarkedText:@"cancel" selectedRange:NSMakeRange(6, 0) replacementRange:NSMakeRange(NSNotFound, 0)];
        [view unmarkText];
        NSCAssert(!view.hasMarkedText && captured.length == [text lengthOfBytesUsingEncoding:NSUTF8StringEncoding], @"Cancelled composition reached the PTY");
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
        brig_terminal_destroy(pointer);
        puts("PASS native fonts: preferred font, bundled JetBrains Mono fallback, monospace metrics and 14pt size");
        puts("PASS native composition: marked text, UTF-8 commit, cancellation and F2 encoding");
    }
    return 0;
}
