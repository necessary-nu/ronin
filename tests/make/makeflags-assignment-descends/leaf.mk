all: ; @printf 'FROM=%s ALSO=%s OVER=%s origin=%s\n' '$(FROM)' '$(ALSO)' '$(OVER)' '$(origin FROM)' > seen
