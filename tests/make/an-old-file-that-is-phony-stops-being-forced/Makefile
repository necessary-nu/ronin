# What forces a phony is that it does not exist. An -o name has a date, so the
# phony is current, its recipe does not run, and the target reading it is no
# longer dragged along by it.
.PHONY: always
out: in.txt always
	@echo out >> ran
	@touch out
always:
	@echo always >> ran
