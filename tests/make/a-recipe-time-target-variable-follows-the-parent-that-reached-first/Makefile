# The link a prerequisite inherits along is `file->parent', which `update_file'
# sets the first time a name is reached and never moves. A name reached from
# two places inherits from the first of them and from no other, so whether a
# recipe-time assignment reaches it is decided by which parent got there first
# -- not by which parents name it.
#
# `p2' is reached first here, so `shared' inherits from `p2' and takes the
# write. Written the other way round, `shared' would inherit from `p1' and read
# `outer' while `p2' still read `to-p2'.
V = outer
all: p2 p1
p2: setter shared
p1: shared
setter:
	$(eval p2: V = to-p2)
	@echo "setter done" >> log
shared:
	@echo "shared:[$(V)]" >> log
p1:
	@echo "p1:[$(V)]" >> log
p2:
	@echo "p2:[$(V)]" >> log
