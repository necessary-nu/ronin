define outer
 define inner
  X = Y
 endef
endef
$(eval $(outer))
all: ; @printf '[%s]\n' '$(inner)' > out
