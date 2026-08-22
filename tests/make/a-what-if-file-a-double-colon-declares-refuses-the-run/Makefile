# `-W out` where `out` is a double-colon target refuses the whole run. GNU Make
# stamps a `-W` name by ENTERING it (main.c), and `enter_file` (file.c) hands
# back the entry it found only when that entry is not a double-colon target — so
# the stamp appends a fresh `struct file` to the chain, carrying a date and no
# recipe, and `remake_file` complains over a file it cannot make. Nothing is
# built: `app` is older than `out` and stays where it was.
all: app
app: out
	touch app
out:: a
	touch out
out:: b
	touch out
