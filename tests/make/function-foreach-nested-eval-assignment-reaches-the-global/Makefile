# Two bindings of one name, and the assignment still lands outside both.
trace := $(foreach n,1 2,$(foreach n,x y,$(eval n = deep)[$(n)])(outer $(n)))
all: ; @printf '%s\n' '$(trace)' 'after [$(n)] $(origin n)' > out
