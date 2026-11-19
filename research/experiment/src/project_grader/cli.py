"""Runs `python -m <module> <argv...>` for a hidden test, in the Python profile's child: the tree on the path, the
test's working directory's files as the current directory. Arguments: the tree, the working directory, the
module, then the command's own arguments."""

import os
import runpy
import sys

tree, work, module = sys.argv[1], sys.argv[2], sys.argv[3]
sys.argv = [module] + sys.argv[4:]
sys.path.insert(0, tree)
os.chdir(work)
runpy.run_module(module, run_name="__main__", alter_sys=True)
