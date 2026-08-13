all: ; @printf '[%s]\n' '$(intcmp 007,7)' '$(intcmp -0,0)' '$(intcmp 000,0)' '$(intcmp +5,5)' \
 '$(intcmp   12  ,12)' '$(intcmp -5,-6,lt,eq,gt)' '$(intcmp -10,-9,lt,eq,gt)' \
 '$(intcmp 99999999999999999999999,99999999999999999999998,lt,eq,gt)' > out
