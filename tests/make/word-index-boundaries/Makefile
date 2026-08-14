FOO = a b c d e f g h i j
all:
	@printf "%s\n" "[$(word 11,$(FOO))]" "[$(word  3 ,$(FOO))]" "[$(word +3,$(FOO))]" "[$(wordlist 2,0,$(FOO))]" "[$(wordlist 5,2,$(FOO))]" "[$(wordlist 2,99,$(FOO))]" > out
