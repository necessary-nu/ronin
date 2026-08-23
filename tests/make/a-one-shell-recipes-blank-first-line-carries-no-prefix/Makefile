# Which line a `.ONESHELL` recipe reads its `[@+-]` prefixes off, when the first
# one expanded to nothing.
#
# `chop_commands` leaves the recipe as ONE line, so the scan that fills
# `lines_flags` runs over the front of the whole text — and the front of this
# recipe is a `$`, so the recipe carries no prefix at all. `strict` therefore
# runs under the `-ec` that `.POSIX:` gives it, stops at the line that fails,
# and never writes its file.
#
# `forgiven` is the control beside it and the same recipe with the blank line
# taken out: there the `-` really is on the first line, the script is started
# without `-e`, and the file after the failure is written.
#
# A build that drops the line that expanded to nothing reads `-false` as the
# first line of `strict` too, and writes both files.
.POSIX:
.ONESHELL:
EMPTY =
all: forgiven strict
forgiven:
	-false
	echo forgiven > forgiven.out
strict:
	$(EMPTY)
	-false
	echo strict > strict.out
