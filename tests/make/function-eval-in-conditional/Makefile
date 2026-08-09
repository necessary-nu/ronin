define EXTRA
all: more
more: ; @printf '%s\n' 'extra' > more.out
endef
all: ; @printf '%s\n' 'base' > out
ifdef BAR
$(eval $(EXTRA))
endif
