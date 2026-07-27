#include <errno.h>
#include <stdarg.h>
#include <stdlib.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include "util.h"

extern const char *argv0;

// [spec:samurai:def:util.vwarn-fn]
// [spec:samurai:sem:util.vwarn-fn]
static void
vwarn(const char *fmt, va_list ap)
{
	fprintf(stderr, "%s: ", argv0);
	vfprintf(stderr, fmt, ap);
	if (fmt[0] && fmt[strlen(fmt) - 1] == ':') {
		putc(' ', stderr);
		perror(NULL);
	} else {
		putc('\n', stderr);
	}
}

// [spec:samurai:def:util.warn-fn]
// [spec:samurai:sem:util.warn-fn]
void
warn(const char *fmt, ...)
{
	va_list ap;

	va_start(ap, fmt);
	vwarn(fmt, ap);
	va_end(ap);
}

// [spec:samurai:def:util.fatal-fn]
// [spec:samurai:sem:util.fatal-fn]
void
fatal(const char *fmt, ...)
{
	va_list ap;

	va_start(ap, fmt);
	vwarn(fmt, ap);
	va_end(ap);
	exit(1);
}

// [spec:samurai:def:util.xmalloc-fn]
// [spec:samurai:sem:util.xmalloc-fn]
void *
xmalloc(size_t n)
{
	void *p;

	p = malloc(n);
	if (!p)
		fatal("malloc:");

	return p;
}

// [spec:samurai:def:util.reallocarray-fn]
// [spec:samurai:sem:util.reallocarray-fn]
static void *
reallocarray_(void *p, size_t n, size_t m)
{
	if (m && n > SIZE_MAX / m) {
		errno = ENOMEM;
		return NULL;
	}
	return realloc(p, n * m);
}

// [spec:samurai:def:util.xreallocarray-fn]
// [spec:samurai:sem:util.xreallocarray-fn]
void *
xreallocarray(void *p, size_t n, size_t m)
{
	p = reallocarray_(p, n, m);
	if (!p)
		fatal("reallocarray:");

	return p;
}

// [spec:samurai:def:util.xmemdup-fn]
// [spec:samurai:sem:util.xmemdup-fn]
char *
xmemdup(const char *s, size_t n)
{
	char *p;

	p = xmalloc(n);
	memcpy(p, s, n);

	return p;
}

// [spec:samurai:def:util.xasprintf-fn]
// [spec:samurai:sem:util.xasprintf-fn]
int
xasprintf(char **s, const char *fmt, ...)
{
	va_list ap;
	int ret;
	size_t n;

	va_start(ap, fmt);
	ret = vsnprintf(NULL, 0, fmt, ap);
	va_end(ap);
	if (ret < 0)
		fatal("vsnprintf:");
	n = ret + 1;
	*s = xmalloc(n);
	va_start(ap, fmt);
	ret = vsnprintf(*s, n, fmt, ap);
	va_end(ap);
	if (ret < 0 || (size_t)ret >= n)
		fatal("vsnprintf:");

	return ret;
}

// [spec:samurai:def:util.bufadd-fn]
// [spec:samurai:sem:util.bufadd-fn]
void
bufadd(struct buffer *buf, char c)
{
	if (buf->len >= buf->cap) {
		buf->cap = buf->cap ? buf->cap * 2 : 1 << 8;
		buf->data = realloc(buf->data, buf->cap);
		if (!buf->data)
			fatal("realloc:");
	}
	buf->data[buf->len++] = c;
}

// [spec:samurai:def:util.mkstr-fn]
// [spec:samurai:sem:util.mkstr-fn]
struct string *
mkstr(size_t n)
{
	struct string *str;

	str = xmalloc(sizeof(*str) + n + 1);
	str->n = n;

	return str;
}

// [spec:samurai:def:util.delevalstr-fn]
// [spec:samurai:sem:util.delevalstr-fn]
void
delevalstr(void *ptr)
{
	struct evalstring *str = ptr, *p;

	while (str) {
		p = str;
		str = str->next;
		if (p->var)
			free(p->var);
		else
			free(p->str);
		free(p);
	}
}

// [spec:samurai:def:util.canonpath-fn]
// [spec:samurai:sem:util.canonpath-fn]
void
canonpath(struct string *path)
{
	char *component[60];
	int n;
	char *s, *d, *end;

	if (path->n == 0)
		fatal("empty path");
	s = d = path->s;
	end = path->s + path->n;
	n = 0;
	if (*s == '/') {
		++s;
		++d;
	}
	while (s < end) {
		switch (s[0]) {
		case '/':
			++s;
			continue;
		case '.':
			switch (s[1]) {
			case '\0': case '/':
				s += 2;
				continue;
			case '.':
				if (s[2] != '/' && s[2] != '\0')
					break;
				if (n > 0) {
					d = component[--n];
				} else {
					*d++ = s[0];
					*d++ = s[1];
					*d++ = s[2];
				}
				s += 3;
				continue;
			}
		}
		if (n == countof(component))
			fatal("path has too many components: %s", path->s);
		component[n++] = d;
		while (*s != '/' && *s != '\0')
			*d++ = *s++;
		*d++ = *s++;
	}
	if (d == path->s) {
		*d++ = '.';
		*d = '\0';
	} else {
		*--d = '\0';
	}
	path->n = d - path->s;
}

// [spec:samurai:def:util.writefile-fn]
// [spec:samurai:sem:util.writefile-fn]
int
writefile(const char *name, struct string *s)
{
	FILE *f;
	int ret;

	f = fopen(name, "w");
	if (!f) {
		warn("open %s:", name);
		return -1;
	}
	ret = 0;
	if (s && (fwrite(s->s, 1, s->n, f) != s->n || fflush(f) != 0)) {
		warn("write %s:", name);
		ret = -1;
	}
	fclose(f);

	return ret;
}
