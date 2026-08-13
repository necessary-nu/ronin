V = a/b
all: ; @printf '[%s]\n' '$(call notdir,$$(V))' '$(call foreach,v,1 2,x$(v))' \
 '$(call foreach,v,1 2,$$(v)y)' '$(call let,q r,1 2 3,$$(q)/$$(r))' \
 '$(call if,1,$$(V),n)' '$(if 1,$$(V))' '$(call intcmp,1,1,lt,$$(V),gt)' > out
