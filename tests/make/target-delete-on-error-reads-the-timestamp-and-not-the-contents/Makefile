# The test is the timestamp, exactly as GNU Make's delete_target makes it: this
# recipe rewrote the file and then put the old timestamp back, and neither tool
# can tell that from a recipe that never touched it.
.DELETE_ON_ERROR:
all: out
out: dep ; echo new > $@ ; touch -d '2020-01-01 00:00' $@ ; exit 1
