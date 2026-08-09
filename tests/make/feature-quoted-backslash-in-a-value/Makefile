# Backslashes and quotes in a variable's value are not make's business: they
# reach the recipe unchanged, for the shell to make of what it will.
FONTS = nicefont
DEFINES = -DPATH=\".:$(FONTS)\"

all: ; @printf '%s\n' '$(DEFINES)' > out
