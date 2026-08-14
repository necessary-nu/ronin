# `undefine` reaches the global binding, not the loop's.
z = outer
inner := $(foreach z,a b,$(eval undefine z)[$(z)][$(origin z)])
all: ; @printf '%s\n' '$(inner)' 'after [$(z)] $(origin z)' > out
