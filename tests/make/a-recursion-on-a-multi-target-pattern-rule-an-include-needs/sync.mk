sync:
	@mkdir -p gen
	@printf 'GENERATED := from-sync\n' > gen/auto.conf
	@: > gen/auto.conf.cmd
