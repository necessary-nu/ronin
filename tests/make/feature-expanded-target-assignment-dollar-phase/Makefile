# Dollars surviving expansion of the colon-bearing word belong to the target
# assignment phase. This applies independently to the name and value.
X = good
NAME = V
VALUE_HEAD = value-target: V=$$(X)
NAME_HEAD = name-target: $$(NAME)=named

$(VALUE_HEAD)
$(NAME_HEAD)

.PHONY: all value-target name-target
all: value-target name-target ; @printf '%s\n' all > all-out
value-target: ; @printf '<%s>\n' '$(V)' > value-out
name-target: ; @printf '<%s>\n' '$(V)' > name-out
