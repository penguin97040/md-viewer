typedef struct {int valid,cancelled,x,y,pid;} MDDragResult;
typedef struct {int x,y,w,h;} MDArea;
void md_install(void);
int md_start(void);
MDDragResult md_end(void);
MDArea md_placement(int x,int y);

void md_position(MDArea area);
MDDragResult md_client_point(int x,int y);
int md_visible(void);
