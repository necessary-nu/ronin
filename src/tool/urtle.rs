const ENCODED: &str = concat!(
    " 13 ,3;2!2;\n8 ,;<11!;\n5 `'<10!(2`'2!\n11 ,6;, `\\. `\\9 .,c13$ec,.\n6 ",
    ",2;11!>; `. ,;!2> .e8$2\".2 \"?7$e.\n <:<8!'` 2.3,.2` ,3!' ;,(?7\";2!2'<",
    "; `?6$PF ,;,\n2 `'4!8;<!3'`2 3! ;,`'2`2'3!;4!`2.`!;2 3,2 .<!2'`).\n5 3`5",
    "'2`9 `!2 `4!><3;5! J2$b,`!>;2!:2!`,d?b`!>\n26 `'-;,(<9!> $F3 )3.:!.2 d\"",
    "2 ) !>\n30 7`2'<3!- \"=-='5 .2 `2-=\",!>\n25 .ze9$er2 .,cd16$bc.'\n22 .e",
    "14$,26$.\n21 z45$c .\n20 J50$c\n20 14$P\"`?34$b\n20 14$ dbc `2\"?22$?7$c",
    "\n20 ?18$c.6 4\"8?4\" c8$P\n9 .2,.8 \"20$c.3 ._14 J9$\n .2,2c9$bec,.2 `?",
    "21$c.3`4%,3%,3 c8$P\"\n22$c2 2\"?21$bc2,.2` .2,c7$P2\",cb\n23$b bc,.2\"2",
    "?14$2F2\"5?2\",J5$P\" ,zd3$\n24$ ?$3?%3 `2\"2?12$bcucd3$P3\"2 2=7$\n23$P",
    "\" ,3;<5!>2;,. `4\"6?2\"2 ,9;, `\"?2$\n",
);

pub(crate) fn decode() -> Vec<u8> {
    let mut output = Vec::new();
    let mut count = 0_usize;
    for byte in ENCODED.bytes() {
        if byte.is_ascii_digit() {
            count = count * 10 + usize::from(byte - b'0');
        } else {
            output.extend(std::iter::repeat_n(byte, count.max(1)));
            count = 0;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    #[test]
    #[allow(
        clippy::naive_bytecount,
        reason = "the tiny fixed mascot fixture does not justify another runtime dependency"
    )]
    fn decodes_the_hidden_ninja_mascot() {
        let mascot = super::decode();
        assert!(mascot.starts_with(b"              ,;;;!!;;\n"));
        assert!(mascot.ends_with(b"`\"?$$\n"));
        assert_eq!(mascot.iter().filter(|byte| **byte == b'\n').count(), 23);
    }
}
