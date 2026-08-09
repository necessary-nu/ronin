# -include and sinclude say the file may be absent; neither the read nor the
# exit status may complain about it.
-include missing-one.mk
sinclude missing-two.mk

all: ; @printf '%s\n' ok > out
