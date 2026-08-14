define plain
a\
  b
endef
define kept
a\\\
  b
endef
define dollared
x$\
  y
endef
all: ; @printf '%s\n' '$(plain)' '$(kept)' '$(dollared)' > out
