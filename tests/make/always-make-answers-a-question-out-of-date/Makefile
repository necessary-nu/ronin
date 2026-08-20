# A question under `-B` is answered about the same graph the build would run,
# so a goal that is current without the switch is out of date with it and the
# run ends with the status that says so. Nothing is built either way.
out: src
	@printf 'ran\n' > ran
	@touch out
