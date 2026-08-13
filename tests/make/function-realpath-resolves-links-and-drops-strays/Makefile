all: ; @printf '%s\n' '$(realpath //)' '[$(realpath .dangling)]' '[$(realpath nosuch)]' '[$(realpath )]' \
 '$(if $(filter $(CURDIR)/d/f,$(realpath link)),same,differ)' \
 '$(if $(filter $(CURDIR)/d/f,$(realpath d//f) $(realpath d/../d/f)),same,differ)' \
 '$(words $(realpath d/f nosuch d/sub))' > out
