## Where things are

The task is the file TASKS.md on the hub's main branch, which only the task setter writes. A change to the task arrives as a new version of TASKS.md on main, whose last line lists the changes released so far. The library counts once it is on the hub's main: a pull request gets there only when someone merges it with pr_merge, which needs its checks to pass, a non-author's approval of its current head, and no one else's latest review asking for changes. Others' work reaches you, and yours reaches them, only when you sync.
