y = outer
inner := $(let y z,1 2,[$(y)]$(eval y = hijack)[$(y)][$(origin y)])
all: ; @printf '%s\n' '$(inner)' 'after [$(y)] $(origin y)' > out
