MAKEFLAGS += --shuffle=none
all: p1 p2 p3
	@printf '%s\n' '$(MAKEFLAGS)' > flags
p1 p2 p3: ; @printf '%s\n' '$@' >> order
