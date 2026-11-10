#import <AppKit/AppKit.h>
#include "WindowAppearance.h"

void brig_window_set_dark_appearance(void *pointer) {
    NSCAssert(NSThread.isMainThread, @"Window appearance must be set on the main thread");
    NSWindow *window = (__bridge NSWindow *)pointer;
    window.subtitle = @"Agent Sandbox";
    window.styleMask &= ~NSWindowStyleMaskFullSizeContentView;
    window.appearance = [NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
    window.titlebarAppearsTransparent = NO;
    window.backgroundColor = [NSColor colorWithSRGBRed:18.0 / 255.0 green:18.0 / 255.0 blue:18.0 / 255.0 alpha:1];
}
