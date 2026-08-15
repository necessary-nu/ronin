# The two names are reached through rules of their own rather than side by
# side on one prerequisite list. It changes nothing: a.c is current, so it
# is a.h that reaches the rule needing to be made.
all: x y
x: a.c ; @echo x
y: a.h ; @echo y
%.c %.h: %.in ; echo 'at=$@ stem=$*' >> log ; cat $< > $*.c ; cat $< > $*.h
