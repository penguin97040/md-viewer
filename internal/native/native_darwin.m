//go:build darwin && cgo && !bindings

#import <Cocoa/Cocoa.h>
#import <ApplicationServices/ApplicationServices.h>
#import <objc/runtime.h>
#import <pthread.h>
#import <string.h>
#import "native_darwin.h"

static pthread_mutex_t md_mutex=PTHREAD_MUTEX_INITIALIZER;
static MDDragResult md_result;
static BOOL md_watching=NO;
static NSMutableDictionary *md_original;
static IMP md_next_original;

// Read Escape in the application's own event loop. This does not install a
// global event tap or ask for accessibility/input-monitoring permission.
static NSEvent *md_next(id self,SEL cmd,NSEventMask mask,NSDate *date,NSString *mode,BOOL dequeue) {
 NSEvent *event=((NSEvent *(*)(id,SEL,NSEventMask,NSDate*,NSString*,BOOL))md_next_original)(self,cmd,mask,date,mode,dequeue);
 if(event.type==NSEventTypeKeyDown&&event.keyCode==53) {pthread_mutex_lock(&md_mutex);if(md_watching)md_result.cancelled=1;pthread_mutex_unlock(&md_mutex);}
 return event;
}
static int md_pid(NSPoint point) {
 NSArray *windows=(NSArray*)CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly|kCGWindowListExcludeDesktopElements,kCGNullWindowID);int pid=0;
 for(NSDictionary *window in windows) {
  if([window[(id)kCGWindowLayer] intValue]!=0||[window[(id)kCGWindowAlpha] doubleValue]==0)continue;
  CGRect bounds;if(CGRectMakeWithDictionaryRepresentation((CFDictionaryRef)window[(id)kCGWindowBounds],&bounds)&&CGRectContainsPoint(bounds,CGPointMake(point.x,point.y))) {pid=[window[(id)kCGWindowOwnerPID] intValue];break;}
 }
 [windows release];return pid;
}
static void md_ended(id self,SEL cmd,NSDraggingSession *session,NSPoint point,NSDragOperation operation) {
 CGFloat top=NSMaxY([NSScreen screens][0].frame);NSPoint desktop=NSMakePoint(point.x,top-point.y);
 pthread_mutex_lock(&md_mutex);
 if(md_watching) {md_result.valid=1;md_result.x=(int)desktop.x;md_result.y=(int)desktop.y;md_result.pid=md_pid(desktop);md_watching=NO;}
 pthread_mutex_unlock(&md_mutex);
 Class cls=object_getClass(self);IMP original=NULL;
 while(cls&&!original) {original=[md_original[NSStringFromClass(cls)] pointerValue];cls=class_getSuperclass(cls);}
 if(original)((void(*)(id,SEL,NSDraggingSession*,NSPoint,NSDragOperation))original)(self,cmd,session,point,operation);
}
static void md_connect(NSView *view) {
 SEL sel=@selector(draggingSession:endedAtPoint:operation:);Class cls=object_getClass(view);Method method=class_getInstanceMethod(cls,sel);
 if(method&&method_getImplementation(method)!=(IMP)md_ended&&!md_original[NSStringFromClass(cls)]) {
  IMP original=method_getImplementation(method);md_original[NSStringFromClass(cls)]=[NSValue valueWithPointer:original];
  if(!class_addMethod(cls,sel,(IMP)md_ended,method_getTypeEncoding(method)))class_replaceMethod(cls,sel,(IMP)md_ended,method_getTypeEncoding(method));
 }
 for(NSView *child in view.subviews)md_connect(child);
}
void md_install(void) {
 dispatch_async(dispatch_get_main_queue(),^{
  if(!md_original) {
   md_original=[NSMutableDictionary new];
   SEL sel=@selector(nextEventMatchingMask:untilDate:inMode:dequeue:);Method method=class_getInstanceMethod([NSApplication class],sel);md_next_original=method_getImplementation(method);method_setImplementation(method,(IMP)md_next);
  }
  for(NSWindow *window in NSApp.windows)md_connect(window.contentView);
 });
}
int md_start(void) {pthread_mutex_lock(&md_mutex);memset(&md_result,0,sizeof(md_result));md_watching=YES;pthread_mutex_unlock(&md_mutex);return 1;}
MDDragResult md_end(void) {pthread_mutex_lock(&md_mutex);MDDragResult result=md_result;md_watching=NO;pthread_mutex_unlock(&md_mutex);return result;}
MDArea md_placement(int x,int y) {
 __block MDArea area={0,0,1100,820};
 void(^work)(void)=^{CGFloat top=NSMaxY([NSScreen screens][0].frame);NSPoint point=NSMakePoint(x,top-y);NSScreen *target=[NSScreen screens][0];
  for(NSScreen *screen in [NSScreen screens])if(NSPointInRect(point,screen.frame)){target=screen;break;}
  NSRect r=target.visibleFrame;int left=r.origin.x,upper=top-NSMaxY(r);area.w=MIN(1100,r.size.width);area.h=MIN(820,r.size.height);area.x=MAX(left,MIN(x-80,left+(int)r.size.width-area.w));area.y=MAX(upper,MIN(y-20,upper+(int)r.size.height-area.h));
 };
 if([NSThread isMainThread])work();else dispatch_sync(dispatch_get_main_queue(),work);return area;
}

void md_position(MDArea area) {
 dispatch_async(dispatch_get_main_queue(),^{
  NSWindow *window=NSApp.mainWindow;if(!window)window=NSApp.windows.firstObject;if(!window)return;
  CGFloat top=NSMaxY([NSScreen screens][0].frame);NSRect frame=window.frame;
  frame.origin.x=area.x;frame.origin.y=top-area.y-frame.size.height;
  [window setFrame:frame display:YES];
 });
}
MDDragResult md_client_point(int x,int y) {
 __block MDDragResult result={0,0,0,0,0};
 void(^work)(void)=^{NSWindow *window=NSApp.mainWindow;if(!window)window=NSApp.windows.firstObject;if(!window)return;
 CGFloat top=NSMaxY([NSScreen screens][0].frame);NSPoint point=[window convertPointFromScreen:NSMakePoint(x,top-y)];
 NSView *view=window.contentView;point=[view convertPoint:point fromView:nil];
 result.valid=1;result.x=point.x;result.y=view.isFlipped?point.y:view.bounds.size.height-point.y;
 };
 if([NSThread isMainThread])work();else dispatch_sync(dispatch_get_main_queue(),work);return result;
}
int md_visible(void) {
 __block int visible=0;
 void(^work)(void)=^{NSWindow *window=NSApp.mainWindow;if(!window)window=NSApp.windows.firstObject;
  if(!window||!window.visible||window.miniaturized)return;
  for(NSScreen *screen in [NSScreen screens])if(NSIntersectsRect(window.frame,screen.visibleFrame)){visible=1;break;}
 };
 if([NSThread isMainThread])work();else dispatch_sync(dispatch_get_main_queue(),work);return visible;
}
