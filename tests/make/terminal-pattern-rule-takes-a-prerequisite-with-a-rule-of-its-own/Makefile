# A terminal rule's prerequisite need not be on disk when the rule is chosen:
# a name some rule promises to make ought to exist, and the bar the terminal
# rule sets is only on searching for an implicit rule it has not got.
all: hello.z
%.z:: test.x ; cat $< > $@
test.x: ; echo made-x > $@
