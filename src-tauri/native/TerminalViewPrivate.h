#import <AppKit/AppKit.h>
#import <CoreText/CoreText.h>
#include <ghostty/vt.h>
#include "TerminalView.h"
#include "TerminalTheme.h"

NSFont *brigTerminalFont(NSString *preferredName, NSURL *fallbackURL);

typedef NS_ENUM(NSUInteger, BrigCompositionState) {
    BrigCompositionIdle,
    BrigCompositionMarked,
};

typedef NS_ENUM(NSUInteger, BrigSelectionState) {
    BrigSelectionIdle,
    BrigSelectionAnchored,
    BrigSelectionDragging,
};

@interface BrigTerminalView : NSView {
@public
    GhosttyTerminal terminal;
    GhosttyRenderState renderState;
    GhosttyRenderStateRowIterator rowIterator;
    GhosttyRenderStateRowCells cells;
    GhosttyKeyEncoder keyEncoder;
    GhosttyKeyEvent keyEvent;
    uint64_t identifier;
    BrigTerminalInput input;
    BrigTerminalResize resize;
    NSFont *font;
    CGFloat cellWidth;
    CGFloat cellHeight;
    uint16_t columns;
    uint16_t rows;
    uint16_t pixelWidth;
    uint16_t pixelHeight;
    NSMutableAttributedString *markedText;
    BrigCompositionState compositionState;
    NSRange markedSelection;
    GhosttyFormatter formatter;
    GhosttyTrackedGridRef selectionAnchor;
    BrigSelectionState selectionState;
    NSCache<NSString *, id> *glyphCache;
    NSTimer *cursorTimer;
    NSTimer *dragTimer;
    NSPoint dragPoint;
    CGFloat scrollRemainder;
    NSTimeInterval lastAccessibilityUpdate;
}
- (void)updateGrid;
@end
@interface BrigTerminalView (Drawing)
- (NSRect)cursorRectangle;
- (NSString *)formattedText:(BOOL)selectionOnly;
@end
@interface BrigTerminalView (Input) <NSTextInputClient>
- (void)sendData:(NSData *)data;
- (void)endSelectionDrag;
@end
