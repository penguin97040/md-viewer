//go:build linux && cgo && !bindings

package native

/*
#cgo pkg-config: gtk+-3.0 gdk-x11-3.0 x11
#include <gtk/gtk.h>
#include <gdk/gdkx.h>
#include <X11/Xatom.h>
#include <string.h>

typedef struct { int valid, cancelled, x, y, pid; } DragResult;
static GMutex drag_mutex;
static DragResult drag_result;
static int watching;
 static GtkWidget *drag_view;
static gboolean failed(GtkWidget *widget, GdkDragContext *context, GtkDragResult why, gpointer data) {
 g_mutex_lock(&drag_mutex);
 if (watching && why != GTK_DRAG_RESULT_NO_TARGET) drag_result.cancelled=1;
 g_mutex_unlock(&drag_mutex);
 return FALSE;
}
static int window_pid(Display *d,Window window,int depth) {
 Atom atom=XInternAtom(d,"_NET_WM_PID",False),actual;int format;unsigned long n,remaining;unsigned char *value=NULL;
 if(XGetWindowProperty(d,window,atom,0,1,False,XA_CARDINAL,&actual,&format,&n,&remaining,&value)==Success&&value) {
  int pid=(actual==XA_CARDINAL&&format==32&&n==1)?(int)*(unsigned long*)value:0;XFree(value);if(pid)return pid;
 }
 if(depth==0)return 0;Window root,parent,*children=NULL;unsigned int count=0;int pid=0;
 if(XQueryTree(d,window,&root,&parent,&children,&count)) {
  for(unsigned int i=0;i<count&&i<128&&!pid;i++)pid=window_pid(d,children[i],depth-1);
  if(children)XFree(children);
 }
 return pid;
}
static int pointer_pid_unchecked(GdkDisplay *display) {
 if (!GDK_IS_X11_DISPLAY(display)) return 0;
 Display *d=GDK_DISPLAY_XDISPLAY(display);Window root=DefaultRootWindow(d),child,rr;int rx,ry,wx,wy;unsigned int mask;
 if (!XQueryPointer(d,root,&rr,&child,&rx,&ry,&wx,&wy,&mask)) return 0;
 // GTK's drag icon is a separate override-redirect window owned by the
 // source. Ignore it and hit-test the normal windows beneath the pointer.
 Window parent,*children=NULL;unsigned int count=0;
 if(!XQueryTree(d,root,&rr,&parent,&children,&count))return 0;
 Window target=None;
 for(int i=(int)count-1;i>=0;i--) {
  XWindowAttributes attrs;if(!XGetWindowAttributes(d,children[i],&attrs)||attrs.map_state!=IsViewable||attrs.override_redirect)continue;
  int left,top;Window unused;XTranslateCoordinates(d,children[i],root,0,0,&left,&top,&unused);
  if(rx>=left&&ry>=top&&rx<left+attrs.width&&ry<top+attrs.height){target=children[i];break;}
 }
 if(children)XFree(children);child=target;
 // Decoration windows can cover the client under a pointer grab and have
 // no process id. Find the managed client within the hit frame instead.
 return target?window_pid(d,target,3):0;
}
static int pointer_pid(GdkDisplay *display) {
 if(!GDK_IS_X11_DISPLAY(display))return 0;
 gdk_x11_display_error_trap_push(display);int pid=pointer_pid_unchecked(display);gdk_x11_display_error_trap_pop_ignored(display);return pid;
}
static void ended(GtkWidget *widget, GdkDragContext *context, gpointer data) {
 GdkDisplay *display=gtk_widget_get_display(widget);GdkDevice *device=gdk_drag_context_get_device(context);int x=0,y=0;gdk_device_get_position(device,NULL,&x,&y);
 int pid=pointer_pid(display);
 g_mutex_lock(&drag_mutex);
 if(watching) { drag_result.valid=GDK_IS_X11_DISPLAY(display);drag_result.x=x;drag_result.y=y;drag_result.pid=pid;watching=0; }
 g_mutex_unlock(&drag_mutex);
}
static void connect_view(GtkWidget *widget,gpointer data) {
 if(strstr(G_OBJECT_TYPE_NAME(widget),"WebKitWebView")&&!g_object_get_data(G_OBJECT(widget),"md-drag-observer")) {
  g_object_set_data(G_OBJECT(widget),"md-drag-observer",GINT_TO_POINTER(1));
 drag_view=widget;g_object_add_weak_pointer(G_OBJECT(widget),(gpointer*)&drag_view);
  g_signal_connect(widget,"drag-failed",G_CALLBACK(failed),NULL);
  g_signal_connect(widget,"drag-end",G_CALLBACK(ended),NULL);
 }
 if(GTK_IS_CONTAINER(widget))gtk_container_foreach(GTK_CONTAINER(widget),connect_view,NULL);
}
static gboolean install_views(gpointer data) {GList *windows=gtk_window_list_toplevels();for(GList *i=windows;i;i=i->next)connect_view(i->data,NULL);g_list_free(windows);return G_SOURCE_REMOVE;}
static void install(void) {g_idle_add(install_views,NULL);}
static int start(void) {g_mutex_lock(&drag_mutex);memset(&drag_result,0,sizeof(drag_result));watching=1;g_mutex_unlock(&drag_mutex);return 1;}
static DragResult end(void) {g_mutex_lock(&drag_mutex);DragResult r=drag_result;watching=0;g_mutex_unlock(&drag_mutex);return r;}
typedef struct {int x,y,w,h;} Area;
static Area compute_placement(int x,int y) {
 GdkDisplay *display=gdk_display_get_default();Area r={0,0,1100,820};if(!display)return r;
 GdkMonitor *monitor=gdk_display_get_monitor_at_point(display,x,y);if(!monitor)return r;GdkRectangle rect;gdk_monitor_get_workarea(monitor,&rect);
 r.w=MIN(1100,rect.width);r.h=MIN(820,rect.height);r.x=MAX(rect.x,MIN(x-80,rect.x+rect.width-r.w));r.y=MAX(rect.y,MIN(y-20,rect.y+rect.height-r.h));return r;
}
typedef struct { GMutex mutex; GCond cond; int done,x,y; Area area; } PlaceRequest;
static gboolean place_on_main(gpointer data) {
 PlaceRequest *r=data;Area area=compute_placement(r->x,r->y);
 g_mutex_lock(&r->mutex);r->area=area;r->done=1;g_cond_signal(&r->cond);g_mutex_unlock(&r->mutex);return G_SOURCE_REMOVE;
}
static Area placement(int x,int y) {
 PlaceRequest r;memset(&r,0,sizeof(r));g_mutex_init(&r.mutex);g_cond_init(&r.cond);r.x=x;r.y=y;
 g_mutex_lock(&r.mutex);g_idle_add(place_on_main,&r);while(!r.done)g_cond_wait(&r.cond,&r.mutex);g_mutex_unlock(&r.mutex);
 Area area=r.area;g_cond_clear(&r.cond);g_mutex_clear(&r.mutex);return area;
}
typedef struct {GMutex mutex;GCond cond;int done,kind,x,y;DragResult result;} PointRequest;
static gboolean point_on_main(gpointer data) {
 PointRequest *r=data;DragResult result={0};
 if(drag_view) {GdkDisplay *display=gtk_widget_get_display(drag_view);
  if(r->kind==1&&GDK_IS_X11_DISPLAY(display)) {GdkDevice *device=gdk_seat_get_pointer(gdk_display_get_default_seat(display));int x,y;gdk_device_get_position(device,NULL,&x,&y);result.valid=1;result.x=x;result.y=y;result.pid=pointer_pid(display);}
  else if(r->kind==2) {GdkWindow *window=gtk_widget_get_window(drag_view);if(window){int x,y;gdk_window_get_origin(window,&x,&y);GtkAllocation a;gtk_widget_get_allocation(drag_view,&a);result.valid=1;result.x=r->x-x-a.x;result.y=r->y-y-a.y;}}
  else if(r->kind==3) {GdkWindow *window=gtk_widget_get_window(drag_view);result.valid=gtk_widget_get_mapped(drag_view)&&window&&!(gdk_window_get_state(window)&GDK_WINDOW_STATE_ICONIFIED);}
 }
 g_mutex_lock(&r->mutex);r->result=result;r->done=1;g_cond_signal(&r->cond);g_mutex_unlock(&r->mutex);return G_SOURCE_REMOVE;
}
static DragResult point_request(int kind,int x,int y) {
 PointRequest r;memset(&r,0,sizeof(r));r.kind=kind;r.x=x;r.y=y;g_mutex_init(&r.mutex);g_cond_init(&r.cond);
 g_mutex_lock(&r.mutex);g_idle_add(point_on_main,&r);while(!r.done)g_cond_wait(&r.cond,&r.mutex);g_mutex_unlock(&r.mutex);
 DragResult result=r.result;g_cond_clear(&r.cond);g_mutex_clear(&r.mutex);return result;
}

*/
import "C"

func Init()       { C.install() }
func Start() bool { return C.start() != 0 }
func End() Result {
	r := C.end()
	return Result{r.valid != 0, r.cancelled != 0, int(r.x), int(r.y), int(r.pid)}
}
func Placement(x, y int) Area {
	r := C.placement(C.int(x), C.int(y))
	return Area{int(r.x), int(r.y), int(r.w), int(r.h)}
}

func Position(area Area) bool { return false }

func Cursor() Result {
	r := C.point_request(1, 0, 0)
	return Result{Valid: r.valid != 0, X: int(r.x), Y: int(r.y), PID: int(r.pid)}
}
func ClientPoint(x, y int) (int, int, bool) {
	r := C.point_request(2, C.int(x), C.int(y))
	return int(r.x), int(r.y), r.valid != 0
}

func Visible() bool { return C.point_request(3, 0, 0).valid != 0 }
