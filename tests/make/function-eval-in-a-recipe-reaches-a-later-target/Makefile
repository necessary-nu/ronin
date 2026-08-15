# A recipe-time target-specific assignment reaches whatever has not been
# expanded yet. The chain fixes the order: already, then setter, then later.
V = outer
X = base
all: later
later: setter
setter: already
already:
	@printf 'V=[%s]\n' '$(V)' > already.out
setter:
	$(eval already: V = too-late)
	$(eval later: V = to-later)
	$(eval later: X += appended)
	$(eval setter: V = to-self)
	@printf 'V=[%s]\n' '$(V)' > setter.out
later:
	@printf 'V=[%s] X=[%s]\n' '$(V)' '$(X)' > later.out
