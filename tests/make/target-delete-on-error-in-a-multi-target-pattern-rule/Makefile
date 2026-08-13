# The rule makes both names from one recipe, and each name is protected by the
# pattern that spelled it rather than by the one the search matched.
.DELETE_ON_ERROR:
.PRECIOUS: %.kept
all: foo.gone foo.kept
%.gone %.kept: ; echo G > $*.gone ; echo K > $*.kept ; exit 1
