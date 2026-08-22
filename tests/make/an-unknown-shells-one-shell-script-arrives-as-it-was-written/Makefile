# What a shell GNU Make does not know is handed, written down. The first line
# loses its blanks and its prefixes wherever the recipe goes — `start_job_command`
# (job.c) skips them off the front of the script and stops at the first newline
# — and every line below it keeps both, indentation included, because a shell
# Make cannot name may be one that counts columns.
SHELL = ./capture
.ONESHELL:
all:
	-@ touch m1
	  @touch m2
	    touch m3
