.SUFFIXES:
sub/foo.x: baz.z sub/bar.y baz.z ; @printf '%s\n' '@ = $@' '@D = $(@D)' '@F = $(@F)' '* = [$*]' '< = $<' '<D = $(<D)' '<F = $(<F)' '^ = $^' '^D = $(^D)' '^F = $(^F)' '+ = $+' '? = $?' > out
sub/bar.y baz.z: ; @mkdir -p $(@D) && : > $@
