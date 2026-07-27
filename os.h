#include <sys/types.h>

struct string;

// [spec:samurai:def:os.osgetcwd-fn]
// [spec:samurai:sem:os.osgetcwd-fn]
void osgetcwd(char *, size_t);
/* changes the working directory to the given path */
// [spec:samurai:def:os.oschdir-fn]
// [spec:samurai:sem:os.oschdir-fn]
void oschdir(const char *);
/* creates all the parent directories of the given path */
// [spec:samurai:def:os.osmkdirs-fn]
// [spec:samurai:sem:os.osmkdirs-fn]
int osmkdirs(struct string *, _Bool);
/* queries the mtime of a file in nanoseconds since the UNIX epoch */
// [spec:samurai:def:os.osmtime-fn]
// [spec:samurai:sem:os.osmtime-fn]
int64_t osmtime(const char *);
/* queries the number of online processors */
// [spec:samurai:def:os.osnproc-fn]
// [spec:samurai:sem:os.osnproc-fn]
long osnproc(void);
/* spawn a child process */
// [spec:samurai:def:os.osspawn-fn]
// [spec:samurai:sem:os.osspawn-fn]
pid_t osspawn(char *const argv[], int fd);
