# The automatic variables name what this rule declared, not the union of what
# every rule for the same target declared.
out:: old.in new.in | gate.one
	@printf 'one q=[$?] all=[$^] first=[$<] order=[$|]\n' >> log
out:: other.in | gate.two
	@printf 'two q=[$?] all=[$^] first=[$<] order=[$|]\n' >> log
