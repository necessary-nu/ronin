# scan.c, scan.h

> [spec:ronin:def:scan.addstringpart-fn]
> static void addstringpart(struct evalstring ***end, bool var)

> [spec:ronin:sem:scan.addstringpart-fn]
> Allocate one `evalstring` node, set its `next` link to null, write it into
> the link slot addressed by `*end`, and advance `*end` to that new node's
> `next` slot. The node is consequently appended to the caller's expression
> chain in constant time.
>
> If `var` is true, append a terminating NUL to the shared scanner buffer and
> duplicate its complete contents into the node's `var` field. If `var` is
> false, set `var` to null, allocate a `string` whose logical length is the
> current buffer length, copy those bytes into it, append a NUL at its end,
> and store it in `str`. In either case reset the shared buffer length to
> zero. The new node and its owned string/variable copy become owned by the
> expression list; callers must treat the inactive union-like field as
> irrelevant.

> [spec:ronin:def:scan.comment-fn]
> static bool comment(struct scanner *s)

> [spec:ronin:sem:scan.comment-fn]
> Return false without changing scanner state unless the current character is
> `#`. When it is, repeatedly advance one character until `newline` consumes
> a physical line ending, then return true. The `#`, all comment contents,
> and the line ending are consumed; the next scanner character is the first
> character of the following line.
>
> There is intentionally no EOF case. A comment that reaches EOF without a
> newline keeps advancing and testing EOF rather than returning or reporting
> a scanner error.

> [spec:ronin:def:scan.escape-fn]
> static void escape(struct scanner *s, struct evalstring ***end)

> [spec:ronin:sem:scan.escape-fn]
> This routine runs after its caller has consumed `$`; `s->chr` is the
> character immediately following it. For `$`, space, or `:`, append that
> character literally to the shared buffer and consume it. For a CR or LF,
> consume a valid newline and then consume following scanner whitespace; this
> emits no bytes and implements a continued line.
>
> For `{`, first flush a nonempty literal buffer as a constant part. Consume
> the `{`, collect zero or more `isvar` characters (letters/digits, `_`, `-`,
> and `.`) into the buffer, require `}`, consume it, and flush the collected
> bytes as a variable part. Thus `${}` is a variable with an empty name;
> a missing closing brace is `invalid variable name`.
>
> In every other case, flush any pending literal bytes, collect one or more
> consecutive `issimplevar` characters (letters/digits, `_`, and `-`) from
> the current position, and flush them as a variable part. If none is
> available, report `invalid $ escape`. The routine only appends/links parts;
> it does not evaluate them. All scanner errors terminate through
> `scanerror`.

> [spec:ronin:def:scan.issimplevar-fn]
> static int issimplevar(int c)

> [spec:ronin:sem:scan.issimplevar-fn]
> Return a nonzero value exactly when `c` is alphanumeric according to the C
> character-classification routine, or is `_` or `-`; otherwise return zero.
> This predicate defines the characters accepted by unbraced `$name`
> expansions.

> [spec:ronin:def:scan.isvar-fn]
> static int isvar(int c)

> [spec:ronin:sem:scan.isvar-fn]
> Return a nonzero value when `issimplevar(c)` is nonzero or when `c` is `.`;
> otherwise return zero. It is the broader identifier predicate used for
> names and braced variable expansions.

> [spec:ronin:def:scan.name-fn]
> static void name(struct scanner *s)

> [spec:ronin:sem:scan.name-fn]
> Empty the shared scanner buffer, then while the current character satisfies
> `isvar`, append it and advance the scanner. If no characters were accepted,
> terminate with `expected name`. Append a NUL byte to the buffer and consume
> any following scanner whitespace with `space`.
>
> The name remains only in the reusable global buffer; this helper allocates
> no returned copy. Callers that need to keep it must duplicate the buffer
> before another scan operation overwrites it.

> [spec:ronin:def:scan.newline-fn]
> static bool newline(struct scanner *s)

> [spec:ronin:sem:scan.newline-fn]
> If the current character is LF, consume it and return true. If it is CR,
> consume the CR, require the newly current character to be LF, consume that
> LF, and return true; a lone CR reports `expected '\\n' after '\\r'` at the
> position reached after the CR. For every other current character, leave the
> scanner unchanged and return false. Consuming the LF is what advances the
> scanner line number and resets its column.

> [spec:ronin:def:scan.next-fn]
> static int next(struct scanner *s)

> [spec:ronin:sem:scan.next-fn]
> Advance the source position for the current character, then read and return
> the next byte/EOF from the scanner file. If the current character is LF,
> increment `line` and set `col` to 1; otherwise increment `col`. This rule
> applies even when the current character is EOF if a caller advances again.
> Store the `getc` result in `s->chr` before returning it.

> [spec:ronin:def:scan.scanchar-fn]
> void scanchar(struct scanner *s, int c)

> [spec:ronin:sem:scan.scanchar-fn]
> Require the current character to equal `c`; otherwise terminate with
> `expected '<c>'` without consuming it. On success consume that character,
> then consume any following scanner whitespace with `space`. This is used
> for punctuation such as `=` and `:` and therefore permits literal spaces
> and `$`-newline continuations after that punctuation.

> [spec:ronin:def:scan.scanclose-fn]
> void scanclose(struct scanner *s)

> [spec:ronin:sem:scan.scanclose-fn]
> Close `s->f` with `fclose`. Ignore the close result and do not free, clear,
> or otherwise reset the scanner fields; in particular, the borrowed path and
> current-character state are left untouched.

> [spec:ronin:def:scan.scanerror-fn]
> void scanerror(struct scanner *s, const char *fmt, ...)

> [spec:ronin:sem:scan.scanerror-fn]
> Write one diagnostic to standard error in this exact shape:
> `<argv0>: <path>:<line>:<col>: <formatted message>\\n`, where the message is
> formed from `fmt` and its variadic arguments. Immediately terminate the
> process with status 1. The routine does not close the scanner or return to
> its caller.

> [spec:ronin:def:scan.scanindent-fn]
> bool scanindent(struct scanner *s)

> [spec:ronin:sem:scan.scanindent-fn]
> Repeatedly consume scanner whitespace with `space`, remembering whether at
> least one whitespace unit was consumed. If the next text is a comment,
> consume that whole comment and its newline, then repeat so comment-only
> lines are skipped. Otherwise return true only when whitespace was consumed
> and the next character is not a newline; the newline test consumes a
> newline when one is present. If there was no leading whitespace, return
> false without attempting to consume a newline. Consequently an indented
> non-comment line starts a block entry, while a blank or comment-only line
> is not returned as an entry.

> [spec:ronin:def:scan.scaninit-fn]
> void scaninit(struct scanner *s, const char *path)

> [spec:ronin:sem:scan.scaninit-fn]
> Borrow and store `path`, initialize the source position to line 1, column
> 1, open that path for text reading, and make the first `getc` result the
> current character. If opening fails, raise the fatal `open <path>:` error
> and do not return. An initial EOF is stored normally; no special empty-file
> handling is performed. The scanner does not copy the pathname, so its owner
> must keep it valid until `scanclose`/all scanning is complete.

> [spec:ronin:def:scan.scankeyword-fn]
> int scankeyword(struct scanner *s, char **var)

> [spec:ronin:sem:scan.scankeyword-fn]
> Skip top-level blank lines and comments until a token or EOF is found.
> Leading spaces are permitted only when, after `space` consumes them, the
> remainder is a comment or newline; any other indented top-level text is a
> scanner error `unexpected indent`. A `#` at column start is consumed as a
> comment, and CR/LF is consumed as a newline. EOF returns `EOF` without
> assigning `*var`.
>
> Otherwise scan a name and compare it against the sorted fixed set `build`,
> `default`, `include`, `pool`, `rule`, and `subninja` using binary search.
> Return the corresponding token for an exact match. For any other valid
> name, duplicate the NUL-terminated scanner buffer into `*var` and return
> `VARIABLE`; the caller owns that copy. Keyword paths leave `*var`
> unchanged. The scanner's ordinary name handling consumes trailing
> whitespace before the return.

> [spec:ronin:def:scan.scanname-fn]
> char * scanname(struct scanner *s)

> [spec:ronin:sem:scan.scanname-fn]
> Scan one required name with `name`, then duplicate the complete
> NUL-terminated contents of the shared buffer and return that allocation.
> The caller owns the returned name. Trailing scanner whitespace has already
> been consumed by `name`; malformed or empty names terminate through the
> scanner error path.

> [spec:ronin:def:scan.scanner]
> struct scanner {
>   FILE *f;
>   const char *path;
>   int chr, line, col;
> }

> [spec:ronin:def:scan.scannewline-fn]
> void scannewline(struct scanner *s)

> [spec:ronin:sem:scan.scannewline-fn]
> Consume one LF or CRLF using `newline`. If no line ending begins at the
> current character, terminate with `expected newline`; otherwise return with
> the next logical line's first character current.

> [spec:ronin:def:scan.scanpaths-fn]
> void scanpaths(struct scanner *s)

> [spec:ronin:sem:scan.scanpaths-fn]
> Repeatedly call path-mode `scanstring` until it returns null. Append every
> non-null expression to the global `paths` array at index `npaths`, then
> increment `npaths`. Retain all expressions for the caller; this routine
> does not evaluate or free them.
>
> The array capacity is static and persists across calls. Grow it with
> reallocation when full, choosing capacity 32 on the first allocation and
> doubling thereafter. This function deliberately does not reset `npaths`;
> its parsing caller must do so after consuming the accumulated paths.

> [spec:ronin:def:scan.scanpipe-fn]
> int scanpipe(struct scanner *s, int n)

> [spec:ronin:sem:scan.scanpipe-fn]
> If the current character is not `|`, return 0 without changing scanner
> state. Otherwise consume the first pipe. If the next character is not a
> second pipe, this is a single pipe: require bit 1 of `n`, consume following
> scanner whitespace, and return 1; with that bit clear, terminate with
> `expected '||'`.
>
> If a second pipe is current, require bit 2 of `n` before consuming it; with
> that bit clear, terminate with `unexpected '||'`. Consume the second pipe,
> consume following scanner whitespace, and return 2. No other pipe form is
> recognized.

> [spec:ronin:def:scan.scanstring-fn]
> struct evalstring * scanstring(struct scanner *s, bool path)

> [spec:ronin:sem:scan.scanstring-fn]
> Start a new empty linked `evalstring` result and empty the shared buffer.
> Read until a terminator. On `$`, consume it and delegate its following
> syntax to `escape`, which may append literal bytes, append expression
> parts, or fold a continued line. In non-path mode, every other character
> except CR, LF, and EOF is appended literally, including spaces, `:`, and
> `|`. In path mode, a space, `:`, or `|` terminates the string without being
> consumed; CR, LF, and EOF always terminate without being consumed.
>
> At termination, append a constant expression part when the buffer is
> nonempty. In path mode, then consume following scanner whitespace with
> `space`; non-path mode leaves the terminator current. Return the head of
> the expression chain, which is null for an immediately terminated empty
> string. Returned parts own their copied literals/variable names and must be
> consumed or freed by the caller.

> [spec:ronin:def:scan.singlespace-fn]
> static bool singlespace(struct scanner *s)

> [spec:ronin:sem:scan.singlespace-fn]
> For a literal space, consume it and return true. For `$`, consume the `$`
> and attempt `newline`: if it succeeds, return true, treating the
> dollar-newline pair as one whitespace unit. If no newline follows, push the
> newly read current character back into the file stream, set the current
> character back to `$`, and return false. The source column increment made
> while tentatively consuming `$` is intentionally not rolled back.
>
> For every other current character return false with no state change. Tabs
> and other whitespace are not spaces for this scanner.

> [spec:ronin:def:scan.space-fn]
> static bool space(struct scanner *s)

> [spec:ronin:sem:scan.space-fn]
> Attempt one `singlespace`. If it fails, return false. If it succeeds, keep
> consuming `singlespace` units until the first failure, then return true.
> Thus it consumes runs of literal spaces and `$`-newline continuations, but
> no tabs. The failed `$` probe behavior of `singlespace`, including its
> un-restored column advance, is retained.

> [spec:ronin:def:scan.token]
> enum token {
>   BUILD;
>   DEFAULT;
>   INCLUDE;
>   POOL;
>   RULE;
>   SUBNINJA;
>   VARIABLE;
> }
