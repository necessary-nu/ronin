# A required makefile that is not there and has no rule stops the run. With -o
# it does not: update_file_1 returns us_success on file->updated before
# complain() can be reached, so the read carries on without it. The open's own
# warning is printed either way.
all:
	@echo hi > ran
