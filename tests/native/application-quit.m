#import <AppKit/AppKit.h>

extern bool tugboat_install_quit_handler(NSApplicationTerminateReply (*request)(void));
extern void tugboat_reply_to_quit(bool approved);

static unsigned requests = 0;

static NSApplicationTerminateReply requestQuit(void) {
    requests++;
    // Both replies must arrive after AppKit starts waiting.
    tugboat_reply_to_quit(requests == 2);
    if (requests == 1) {
        [NSApp performSelector:@selector(terminate:) withObject:nil afterDelay:0.25];
    }
    return NSTerminateLater;
}

@interface QuitTestDelegate : NSObject <NSApplicationDelegate>
@end

@implementation QuitTestDelegate
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    [NSApp performSelector:@selector(terminate:) withObject:nil afterDelay:0];
}

- (void)applicationWillTerminate:(NSNotification *)notification {
    NSCAssert(requests == 2, @"AppKit must wait for approval and allow a retry after cancellation.");
    puts("Confirmed two quit requests.");
}
@end

int main(void) {
    @autoreleasepool {
        NSApplication *app = NSApplication.sharedApplication;
        [app setActivationPolicy:NSApplicationActivationPolicyProhibited];
        __attribute__((objc_precise_lifetime)) QuitTestDelegate *delegate = [QuitTestDelegate new];
        app.delegate = delegate;
        NSCAssert(tugboat_install_quit_handler(requestQuit), @"Could not install the quit handler.");
        [app run];
    }
}
