# -t over a target -W has already answered about. GNU Make touches nothing: the
# switch makes the target up to date, and an up-to-date target is not touched
# either. Ronin ran the whole recipe, so -t wrote the file's real contents.
pre.out:
	@echo before > pre.out
	@$(MAKE) -f sub.mk sub
	@echo after >> pre.out
