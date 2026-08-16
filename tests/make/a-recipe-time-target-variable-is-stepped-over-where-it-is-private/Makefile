# `private' is answered twice on the way down and the answers differ. A
# recipe-time assignment written `private' is the target's own and reaches no
# prerequisite at all. A public one written on a target still reaches the
# targets beneath a prerequisite that bound the name `private' for itself,
# because a private binding is stepped over rather than stopped at -- `kid'
# reads its own and `gkid' reads the parent's.
#
# `+=' is the other flavour worth pinning: it appends to what the target
# inherits, and the whole appended value travels, not the tail.
V = outer
kid: private V = kidpriv
all: parent
parent: setter kid appendee
kid: setter gkid
gkid: setter
appendee: setter
setter:
	$(eval parent: V = to-parent)
	$(eval parent: private W = hidden)
	$(eval appendee: X += added)
	@echo "setter done" >> log
gkid:
	@echo "gkid:[$(V)] W=[$(W)]" >> log
kid:
	@echo "kid:[$(V)] W=[$(W)]" >> log
appendee:
	@echo "appendee:[$(X)]" >> log
parent:
	@echo "parent:[$(V)] W=[$(W)]" >> log
X = outerX
