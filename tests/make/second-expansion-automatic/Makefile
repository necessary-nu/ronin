.SECONDEXPANSION:
dep.txt: ; echo dep > $@
out: dep.txt
out: $$(patsubst %,%,dep.txt)
	printf '%s|%s\n' '$<' '$^' > $@
