# `start_job_command` scans the EXPANDED text for prefixes and stops at the
# first newline, so a `-` an expansion put on the first line counts as much as
# one written there.
.ONESHELL:
FIRST = -touch m1
all: out
out:
	$(FIRST)
	false
