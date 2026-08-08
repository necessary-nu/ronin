all: out
dir.stamp: ; echo stamp > $@
in.txt: ; echo in > $@
out: in.txt | dir.stamp
	printf 'normal=%s order=%s\n' '$^' '$|' > $@
