#!/bin/sh
# A stand-in for `open` or `xdg-open` in tests of the login link: the "link"
# it is given is the path of a file, and it writes "opened" there. It is a
# checked-in file, not one a test writes just before running it: running a
# file that was only just written can fail with "Text file busy" when other
# threads in the test program are starting processes.
echo opened > "$1"
