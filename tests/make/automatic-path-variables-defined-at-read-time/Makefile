# GNU Make defines the `D` and `F` forms of the automatic variables once, at
# startup, in `define_automatic_variables` (src/variable.c). They are ordinary
# recursive variables whose text names the base form, so they need no rule in
# hand and are readable everywhere a Makefile can look — including at read
# time, before any rule has been chosen. Only the base forms are per-file, set
# by `set_file_variables`, so they read as undefined here.
#
# `|` is the exception with no path forms at all: GNU Make defines neither
# `$(|D)` nor `$(|F)`, so those two read as variables nobody defined.
$(file >out)
$(foreach v,@ % < ? ^ + | *,\
  $(file >>out,base $(v) origin=[$(origin $(v))] flavor=[$(flavor $(v))] value=[$(value $(v))] expand=[$($(v))]))
$(foreach v,@D @F %D %F <D <F ?D ?F ^D ^F +D +F |D |F *D *F,\
  $(file >>out,path $(v) origin=[$(origin $(v))] flavor=[$(flavor $(v))] value=[$(value $(v))] expand=[$($(v))]))

ifdef @D
$(file >>out,ifdef @D is defined)
else
$(file >>out,ifdef @D is undefined)
endif

ifdef @
$(file >>out,ifdef @ is defined)
else
$(file >>out,ifdef @ is undefined)
endif

.PHONY: all
all: ; @:
