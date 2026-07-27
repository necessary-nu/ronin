#include <ctype.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "scan.h"
#include "util.h"

struct evalstring **paths;
size_t npaths;
static struct buffer buf;

// [spec:samurai:def:scan.scaninit-fn]
// [spec:samurai:sem:scan.scaninit-fn]
void
scaninit(struct scanner *s, const char *path)
{
	s->path = path;
	s->line = 1;
	s->col = 1;
	s->f = fopen(path, "r");
	if (!s->f)
		fatal("open %s:", path);
	s->chr = getc(s->f);
}

// [spec:samurai:def:scan.scanclose-fn]
// [spec:samurai:sem:scan.scanclose-fn]
void
scanclose(struct scanner *s)
{
	fclose(s->f);
}

// [spec:samurai:def:scan.scanerror-fn]
// [spec:samurai:sem:scan.scanerror-fn]
void
scanerror(struct scanner *s, const char *fmt, ...)
{
	extern const char *argv0;
	va_list ap;

	fprintf(stderr, "%s: %s:%d:%d: ", argv0, s->path, s->line, s->col);
	va_start(ap, fmt);
	vfprintf(stderr, fmt, ap);
	va_end(ap);
	putc('\n', stderr);
	exit(1);
}

// [spec:samurai:def:scan.next-fn]
// [spec:samurai:sem:scan.next-fn]
static int
next(struct scanner *s)
{
	if (s->chr == '\n') {
		++s->line;
		s->col = 1;
	} else {
		++s->col;
	}
	s->chr = getc(s->f);

	return s->chr;
}

// [spec:samurai:def:scan.issimplevar-fn]
// [spec:samurai:sem:scan.issimplevar-fn]
static int
issimplevar(int c)
{
	return isalnum(c) || c == '_' || c == '-';
}

// [spec:samurai:def:scan.isvar-fn]
// [spec:samurai:sem:scan.isvar-fn]
static int
isvar(int c)
{
	return issimplevar(c) || c == '.';
}

// [spec:samurai:def:scan.newline-fn]
// [spec:samurai:sem:scan.newline-fn]
static bool
newline(struct scanner *s)
{
	switch (s->chr) {
	case '\r':
		if (next(s) != '\n')
			scanerror(s, "expected '\\n' after '\\r'");
		/* fallthrough */
	case '\n':
		next(s);
		return true;
	}
	return false;
}

// [spec:samurai:def:scan.singlespace-fn]
// [spec:samurai:sem:scan.singlespace-fn]
static bool
singlespace(struct scanner *s)
{
	switch (s->chr) {
	case '$':
		next(s);
		if (newline(s))
			return true;
		ungetc(s->chr, s->f);
		s->chr = '$';
		return false;
	case ' ':
		next(s);
		return true;
	}
	return false;
}

// [spec:samurai:def:scan.space-fn]
// [spec:samurai:sem:scan.space-fn]
static bool
space(struct scanner *s)
{
	if (!singlespace(s))
		return false;
	while (singlespace(s))
		;
	return true;
}

// [spec:samurai:def:scan.comment-fn]
// [spec:samurai:sem:scan.comment-fn]
static bool
comment(struct scanner *s)
{
	if (s->chr != '#')
		return false;
	do next(s);
	while (!newline(s));
	return true;
}

// [spec:samurai:def:scan.name-fn]
// [spec:samurai:sem:scan.name-fn]
static void
name(struct scanner *s)
{
	buf.len = 0;
	while (isvar(s->chr)) {
		bufadd(&buf, s->chr);
		next(s);
	}
	if (!buf.len)
		scanerror(s, "expected name");
	bufadd(&buf, '\0');
	space(s);
}

// [spec:samurai:def:scan.scankeyword-fn]
// [spec:samurai:sem:scan.scankeyword-fn]
int
scankeyword(struct scanner *s, char **var)
{
	/* must stay in sorted order */
	static const struct {
		const char *name;
		int value;
	} keywords[] = {
		{"build",    BUILD},
		{"default",  DEFAULT},
		{"include",  INCLUDE},
		{"pool",     POOL},
		{"rule",     RULE},
		{"subninja", SUBNINJA},
	};
	int low = 0, high = countof(keywords) - 1, mid, cmp;

	for (;;) {
		switch (s->chr) {
		case ' ':
			space(s);
			if (!comment(s) && !newline(s))
				scanerror(s, "unexpected indent");
			break;
		case '#':
			comment(s);
			break;
		case '\r':
		case '\n':
			newline(s);
			break;
		case EOF:
			return EOF;
		default:
			name(s);
			while (low <= high) {
				mid = (low + high) / 2;
				cmp = strcmp(buf.data, keywords[mid].name);
				if (cmp == 0)
					return keywords[mid].value;
				if (cmp < 0)
					high = mid - 1;
				else
					low = mid + 1;
			}
			*var = xmemdup(buf.data, buf.len);
			return VARIABLE;
		}
	}
}

// [spec:samurai:def:scan.scanname-fn]
// [spec:samurai:sem:scan.scanname-fn]
char *
scanname(struct scanner *s)
{
	name(s);
	return xmemdup(buf.data, buf.len);
}

// [spec:samurai:def:scan.addstringpart-fn]
// [spec:samurai:sem:scan.addstringpart-fn]
static void
addstringpart(struct evalstring ***end, bool var)
{
	struct evalstring *p;

	p = xmalloc(sizeof(*p));
	p->next = NULL;
	**end = p;
	if (var) {
		bufadd(&buf, '\0');
		p->var = xmemdup(buf.data, buf.len);
	} else {
		p->var = NULL;
		p->str = mkstr(buf.len);
		memcpy(p->str->s, buf.data, buf.len);
		p->str->s[buf.len] = '\0';
	}
	*end = &p->next;
	buf.len = 0;
}

// [spec:samurai:def:scan.escape-fn]
// [spec:samurai:sem:scan.escape-fn]
static void
escape(struct scanner *s, struct evalstring ***end)
{
	switch (s->chr) {
	case '$':
	case ' ':
	case ':':
		bufadd(&buf, s->chr);
		next(s);
		break;
	case '{':
		if (buf.len > 0)
			addstringpart(end, false);
		while (isvar(next(s)))
			bufadd(&buf, s->chr);
		if (s->chr != '}')
			scanerror(s, "invalid variable name");
		next(s);
		addstringpart(end, true);
		break;
	case '\r':
	case '\n':
		newline(s);
		space(s);
		break;
	default:
		if (buf.len > 0)
			addstringpart(end, false);
		while (issimplevar(s->chr)) {
			bufadd(&buf, s->chr);
			next(s);
		}
		if (!buf.len)
			scanerror(s, "invalid $ escape");
		addstringpart(end, true);
	}
}

// [spec:samurai:def:scan.scanstring-fn]
// [spec:samurai:sem:scan.scanstring-fn]
struct evalstring *
scanstring(struct scanner *s, bool path)
{
	struct evalstring *str = NULL, **end = &str;

	buf.len = 0;
	for (;;) {
		switch (s->chr) {
		case '$':
			next(s);
			escape(s, &end);
			break;
		case ':':
		case '|':
		case ' ':
			if (path)
				goto out;
			/* fallthrough */
		default:
			bufadd(&buf, s->chr);
			next(s);
			break;
		case '\r':
		case '\n':
		case EOF:
			goto out;
		}
	}
out:
	if (buf.len > 0)
		addstringpart(&end, 0);
	if (path)
		space(s);
	return str;
}

// [spec:samurai:def:scan.scanpaths-fn]
// [spec:samurai:sem:scan.scanpaths-fn]
void
scanpaths(struct scanner *s)
{
	static size_t max;
	struct evalstring *str;

	while ((str = scanstring(s, true))) {
		if (npaths == max) {
			max = max ? max * 2 : 32;
			paths = xreallocarray(paths, max, sizeof(paths[0]));
		}
		paths[npaths++] = str;
	}
}

// [spec:samurai:def:scan.scanchar-fn]
// [spec:samurai:sem:scan.scanchar-fn]
void
scanchar(struct scanner *s, int c)
{
	if (s->chr != c)
		scanerror(s, "expected '%c'", c);
	next(s);
	space(s);
}

// [spec:samurai:def:scan.scanpipe-fn]
// [spec:samurai:sem:scan.scanpipe-fn]
int
scanpipe(struct scanner *s, int n)
{
	if (s->chr != '|')
		return 0;
	next(s);
	if (s->chr != '|') {
		if (!(n & 1))
			scanerror(s, "expected '||'");
		space(s);
		return 1;
	}
	if (!(n & 2))
		scanerror(s, "unexpected '||'");
	next(s);
	space(s);
	return 2;
}

// [spec:samurai:def:scan.scanindent-fn]
// [spec:samurai:sem:scan.scanindent-fn]
bool
scanindent(struct scanner *s)
{
	bool indent;

	for (;;) {
		indent = space(s);
		if (!comment(s))
			return indent && !newline(s);
	}
}

// [spec:samurai:def:scan.scannewline-fn]
// [spec:samurai:sem:scan.scannewline-fn]
void
scannewline(struct scanner *s)
{
	if (!newline(s))
		scanerror(s, "expected newline");
}
