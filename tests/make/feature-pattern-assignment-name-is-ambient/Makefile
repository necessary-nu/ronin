# Pattern-specific assignment names are expanded in the ambient scope, not in
# the pattern's previously recorded variable scope, so N is absent here.
%.o: N=P
%.o: $(N)=good

all: ; @:
