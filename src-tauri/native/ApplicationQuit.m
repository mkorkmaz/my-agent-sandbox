#import <AppKit/AppKit.h>
#import <objc/runtime.h>

static NSApplicationTerminateReply (*requestQuit)(void);
static BOOL waitingForReply;

static NSApplicationTerminateReply shouldTerminate(id delegate, SEL selector, NSApplication *application) {
    if (waitingForReply) return NSTerminateLater;
    NSApplicationTerminateReply reply = requestQuit();
    waitingForReply = reply == NSTerminateLater;
    return reply;
}

bool tugboat_install_quit_handler(NSApplicationTerminateReply (*request)(void)) {
    id<NSApplicationDelegate> delegate = NSApp.delegate;
    if (!NSThread.isMainThread || !delegate) return false;
    SEL selector = @selector(applicationShouldTerminate:);
    struct objc_method_description method = protocol_getMethodDescription(@protocol(NSApplicationDelegate), selector, NO, YES);
    if (!method.types || !class_addMethod(object_getClass(delegate), selector, (IMP)shouldTerminate, method.types)) return false;
    requestQuit = request;
    // AppKit caches the optional delegate methods when the delegate is assigned.
    NSApp.delegate = nil;
    NSApp.delegate = delegate;
    return true;
}

void tugboat_reply_to_quit(bool approved) {
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!waitingForReply) return;
        waitingForReply = NO;
        [NSApp replyToApplicationShouldTerminate:approved];
    });
}
