PRE := $(.SHELLSTATUS)
$(shell exit 0)
OK := $(.SHELLSTATUS)
$(shell exit 3)
BAD := $(.SHELLSTATUS)
all: ; @printf '%s\n' 'PRE=[$(PRE)]' 'OK=[$(OK)]' 'BAD=[$(BAD)]' > out
