struct environment;
struct node;

// [spec:samurai:def:parse.parseoptions]
struct parseoptions {
	_Bool dupbuildwarn;
};

void parseinit(void);
void parse(const char *, struct environment *);

extern struct parseoptions parseopts;

/* supported ninja version */
enum {
	ninjamajor = 1,
	ninjaminor = 9,
};

/* execute a function with all default nodes */
void defaultnodes(void(struct node *));
