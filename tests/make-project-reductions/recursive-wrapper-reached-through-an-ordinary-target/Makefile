# zsh's Src/Makemod in miniature: X.mdh depends on X.mdhi depends on X.mdhs,
# and both X.mdh and X.mdhs re-invoke this same Makefile for X.mdh.tmp. The
# two recursive wrappers are linked only through mdhi, which is an ordinary
# target, so Ronin's dependency ordering never sees that mdhs produces what
# mdh's evaluation input needs.
makefile = Makefile

mdh: mdhi
	@$(MAKE) -f $(makefile) tmp
	@mv -f tmp mdh

mdhi: mdhs
	@test -f $@ || echo 'do not delete this file' > $@

mdhs: mdd
	@$(MAKE) -f $(makefile) tmp
	@echo timestamp > $@

tmp:
	@echo content > tmp
