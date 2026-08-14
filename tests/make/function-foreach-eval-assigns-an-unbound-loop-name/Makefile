# The same where the loop variable meant nothing before the loop.
inner := $(foreach w,a b,$(eval w = hijack)[$(w)])
all: ; @printf '%s\n' '$(inner)' 'after [$(w)] $(origin w)' > out
