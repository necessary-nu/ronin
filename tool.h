// [spec:samurai:def:tool.tool]
struct tool {
	const char *name;
	// [spec:samurai:def:tool.tool.run-fn]
	// [spec:samurai:sem:tool.tool.run-fn]
	int (*run)(int, char *[]);
};

const struct tool *toolget(const char *);
