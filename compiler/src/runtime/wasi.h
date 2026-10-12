// What WASI lacks, for a program built for wasm (`--target wasm-server`).
// The compiler pastes this before the runtime. A program there runs one
// request in a sandbox: no processes, signals or sockets of its own. The
// calls that need them fail with ENOSYS, which the runtime reports as an
// ordinary error ("Function not implemented"), so the rest of the runtime
// stays as it is.

#include <errno.h>
#include <stddef.h>
#include <sys/types.h>
#include <unistd.h>

typedef int sig_atomic_t;
#define SIGINT 2
#define SIGPIPE 13
#define SIGTERM 15
#define SIG_IGN ((void (*)(int))1)
#define SIG_DFL ((void (*)(int))0)
static inline void (*signal(int sig, void (*h)(int)))(int) { (void)sig; (void)h; return SIG_DFL; }
static inline int kill(pid_t pid, int sig) { (void)pid; (void)sig; errno = ENOSYS; return -1; }
static inline void tzset(void) {}

// processes
typedef struct { int unused; } posix_spawn_file_actions_t;
static inline int posix_spawn_file_actions_init(posix_spawn_file_actions_t *fa) { (void)fa; return 0; }
static inline int posix_spawn_file_actions_destroy(posix_spawn_file_actions_t *fa) { (void)fa; return 0; }
static inline int posix_spawn_file_actions_adddup2(posix_spawn_file_actions_t *fa, int a, int b) { (void)fa; (void)a; (void)b; return 0; }
static inline int posix_spawn_file_actions_addclose(posix_spawn_file_actions_t *fa, int a) { (void)fa; (void)a; return 0; }
static inline int posix_spawn_file_actions_addchdir_np(posix_spawn_file_actions_t *fa, const char *d) { (void)fa; (void)d; return 0; }
static inline int posix_spawnp(pid_t *pid, const char *file, const posix_spawn_file_actions_t *fa, const void *attr, char *const argv[], char *const envp[]) {
    (void)pid; (void)file; (void)fa; (void)attr; (void)argv; (void)envp;
    return ENOSYS;
}
static inline pid_t waitpid(pid_t pid, int *st, int opts) { (void)pid; (void)st; (void)opts; errno = ENOSYS; return -1; }
#define WNOHANG 1
#define WIFEXITED(s) 1
#define WEXITSTATUS(s) (s)
#define WIFSIGNALED(s) 0
#define WTERMSIG(s) 0

// files
#define mkdtemp lt_wasi_mkdtemp
static inline char *mkdtemp(char *tmpl) { (void)tmpl; errno = ENOSYS; return NULL; }
#define sendfile lt_wasi_sendfile
static inline ssize_t sendfile(int out, int in, off_t *off, size_t n) { (void)out; (void)in; (void)off; (void)n; errno = ENOSYS; return -1; }

// a terminal: never one (reading a password doesn't hide it)
typedef unsigned tcflag_t;
struct termios { tcflag_t c_lflag; };
#define ECHO 8
#define TCSAFLUSH 2
static inline int tcgetattr(int fd, struct termios *t) { (void)fd; (void)t; errno = ENOTTY; return -1; }
static inline int tcsetattr(int fd, int how, const struct termios *t) { (void)fd; (void)how; (void)t; errno = ENOTTY; return -1; }
