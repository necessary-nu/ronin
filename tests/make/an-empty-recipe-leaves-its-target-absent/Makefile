# A recipe that runs nothing still ran, and the target it did not write is
# still not there. GNU Make's `notice_finished_file` leaves such a target's
# timestamp unknown, the next read of it finds nothing, and `update_file_1`
# makes what depends on it for the one reason that outranks every comparison:
# "We must remake if this dep does not exist" (src/remake.c).
#
# So `final` is remade on every invocation however new it is, and `$?` names
# `mid` — a prerequisite that does not exist after being made is newer than the
# target, whatever the clock says.
#
# `bare` is the same target written without the `;`: no recipe at all rather
# than an empty one. GNU Make reaches the same place by a different road, and
# the point of having both here is that the two roads must not diverge.
#
# `kept` is the control. It is on the disk, so the empty recipe leaves a target
# that is there, and nothing above it has any reason to run.
final: mid bare kept
	@echo "ran [$?]" >> log
	@touch final

mid: src ;
bare: src
kept: src ;
