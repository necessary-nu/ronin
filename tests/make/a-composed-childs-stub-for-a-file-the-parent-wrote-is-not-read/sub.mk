out: gen.txt ; @cat gen.txt > out

gen.txt: ; @echo stub-ran > stub.out; false
