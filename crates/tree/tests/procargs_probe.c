// The probe tests/check.rs runs as a check (finding c44e0191291a, decision 53b58f146ce1): it tries every
// way it knows to read another process, its parent, whose environment holds a planted variable. It never
// prints what it reads: one line per way, "<way> open <how much>" or "<way> denied <errno>", and for the
// argument and environment reads whether the plant was among them. It first says whether <pid>, the
// process it reads, is its parent.
//
// With <group>, it first moves into that process group, the target's, the way a check can (the profile
// denies setpgid, not posix_spawn's POSIX_SPAWN_SETPGROUP): it spawns itself there without <group>,
// prints "join open" or "join denied <error>", and waits.
//
//   procargs_probe <the planted variable's name> <pid> [<group>]
#include <errno.h>
#include <libproc.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/sysctl.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

static char marker[256];
static char buf[2 << 20]; // over KERN_ARGMAX, 1 MiB
static size_t argmax = 1 << 20;

static int planted(const char *data, size_t len) {
  size_t m = strlen(marker);
  for (size_t i = 0; i + m <= len; i++)
    if (memcmp(data + i, marker, m) == 0) return 1;
  return 0;
}

// A process's arguments and environment by MIB, the last element the pid. The kernel takes a buffer of
// at most KERN_ARGMAX: a larger one is EINVAL.
static void args(const char *way, int *mib, u_int n) {
  size_t len = argmax;
  if (sysctl(mib, n, buf, &len, NULL, 0) != 0) {
    printf("%s denied %d\n", way, errno);
    return;
  }
  printf("%s open %zu bytes, plant %s\n", way, len, planted(buf, len) ? "found" : "not found");
}

static void args_by_name(const char *way, const char *name, pid_t pid) {
  int mib[CTL_MAXNAME];
  size_t n = CTL_MAXNAME - 1;
  if (sysctlnametomib(name, mib, &n) != 0) {
    printf("%s denied %d (no such name)\n", way, errno);
    return;
  }
  mib[n] = pid;
  args(way, mib, (u_int)n + 1);
}

// A table by MIB: open if it holds more than `one` bytes, more than this process's own entry.
static void table(const char *way, int *mib, u_int n, size_t one) {
  size_t len = 0;
  if (sysctl(mib, n, NULL, &len, NULL, 0) != 0) {
    printf("%s denied %d\n", way, errno);
    return;
  }
  printf("%s %s %zu bytes\n", way, len > one ? "open" : "denied", len);
}

// Whether a list of pids, one a line, holds `pid`.
static int listed(const char *data, size_t len, pid_t pid) {
  for (size_t i = 0; i < len;) {
    char *end;
    long n = strtol(data + i, &end, 10);
    if (end == data + i) break;
    if (n == pid) return 1;
    i = (size_t)(end - data) + 1;
  }
  return 0;
}

// A tool's output, read whole: open if it printed anything, and whether the plant was in it, or with
// `pid`, whether it lists that pid.
static void tool(const char *way, const char *command, pid_t pid) {
  FILE *p = popen(command, "r");
  if (!p) {
    printf("%s denied %d\n", way, errno);
    return;
  }
  size_t len = fread(buf, 1, sizeof buf, p);
  int status = pclose(p);
  if (len == 0) {
    printf("%s denied (no output, status %d)\n", way, status);
    return;
  }
  size_t lines = 0;
  for (size_t i = 0; i < len; i++) lines += buf[i] == '\n';
  if (pid) printf("%s open %zu lines, parent %s\n", way, lines, listed(buf, len, pid) ? "listed" : "not listed");
  else printf("%s open %zu lines, plant %s\n", way, lines, planted(buf, len) ? "found" : "not found");
}

int main(int argc, char **argv) {
  if (argc == 4) {
    setvbuf(stdout, NULL, _IONBF, 0);
    posix_spawnattr_t attr;
    posix_spawnattr_init(&attr);
    posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETPGROUP);
    posix_spawnattr_setpgroup(&attr, (pid_t)atoi(argv[3]));
    char *child_argv[] = {argv[0], argv[1], argv[2], NULL};
    pid_t child;
    int rc = posix_spawn(&child, argv[0], NULL, &attr, child_argv, environ);
    if (rc != 0) {
      printf("join denied %d\n", rc);
      return 0;
    }
    printf("join open\n");
    int status;
    waitpid(child, &status, 0);
    return 0;
  }
  if (argc != 3 || strlen(argv[1]) + 2 > sizeof marker) return 2;
  snprintf(marker, sizeof marker, "%s=", argv[1]);
  pid_t parent = (pid_t)atoi(argv[2]);
  if (parent <= 1) return 2;
  setvbuf(stdout, NULL, _IONBF, 0);
  printf("target %s\n", getppid() == parent ? "parent" : "not-parent");
  int max[] = {CTL_KERN, KERN_ARGMAX};
  int got_max = 0;
  size_t max_len = sizeof got_max;
  if (sysctl(max, 2, &got_max, &max_len, NULL, 0) == 0 && got_max > 0 && (size_t)got_max <= sizeof buf)
    argmax = (size_t)got_max;

  int procargs2[] = {CTL_KERN, KERN_PROCARGS2, parent};
  args("procargs2-mib", procargs2, 3);
  int procargs[] = {CTL_KERN, KERN_PROCARGS, parent};
  args("procargs-mib", procargs, 3);
  args_by_name("procargs2-name", "kern.procargs2", parent);
  args_by_name("procargs-name", "kern.procargs", parent);

  int all[] = {CTL_KERN, KERN_PROC, KERN_PROC_ALL};
  table("kern.proc.all", all, 3, sizeof(struct kinfo_proc));
  int one[] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, parent};
  table("kern.proc.pid", one, 4, 0);

  int pids = proc_listallpids(NULL, 0);
  printf("proc_listallpids %s %d\n", pids > 1 ? "open" : "denied", pids);
  char path[PROC_PIDPATHINFO_MAXSIZE];
  int got = proc_pidpath(parent, path, sizeof path);
  if (got > 0) printf("proc_pidpath open %d bytes\n", got);
  else printf("proc_pidpath denied %d\n", errno);
  struct proc_bsdinfo info;
  got = proc_pidinfo(parent, PROC_PIDTBSDINFO, 0, &info, sizeof info);
  if (got > 0) printf("proc_pidinfo open %d bytes\n", got);
  else printf("proc_pidinfo denied %d\n", errno);

  char command[128];
  snprintf(command, sizeof command, "/bin/ps -E -ww -o command= -p %d 2>/dev/null", (int)parent);
  tool("ps-E", command, 0);
  tool("ps-ax", "/bin/ps -ax -o pid= 2>/dev/null", parent);
  // -a: pgrep leaves out its own ancestors, the parent among them, without it.
  tool("pgrep", "/usr/bin/pgrep -a . 2>/dev/null", parent);
  return 0;
}
