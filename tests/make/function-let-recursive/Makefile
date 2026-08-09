reverse = $(let first rest,$(1),$(if $(rest),$(call reverse,$(rest)) )$(first))
all: ; @printf '[%s]\n' '$(call reverse,moe miny meeny eeny)' > out
