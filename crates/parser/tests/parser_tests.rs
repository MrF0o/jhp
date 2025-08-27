use jhp_parser::{CodeBlock, Parser, blocks_to_js};

fn collect_summaries(blocks: Vec<Box<CodeBlock>>) -> Vec<(char, usize, String, usize)> {
    // (kind, line, content, level)
    blocks
        .into_iter()
        .map(|b| match *b {
            CodeBlock::Html(c) => ('H', c.lineno, c.content, c.level),
            CodeBlock::Javascript(c) => ('J', c.lineno, c.content, c.level),
            CodeBlock::Expression(c) => ('E', c.lineno, c.content, c.level),
        })
        .collect()
}

#[test]
fn parse_html_only() {
    let input = "<h1>Hello</h1>\n<p>World</p>";
    let mut p = Parser::new(input);
    let res = p.parse();
    let s = collect_summaries(res.blocks);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].0, 'H');
    assert_eq!(s[0].1, 1);
    assert_eq!(s[0].3, 0);
    assert_eq!(s[0].2, input);
}

#[test]
fn parse_simple_js_block() {
    let input = "<? let a = 1; ?>";
    let mut p = Parser::new(input);
    let res = p.parse();
    let s = collect_summaries(res.blocks);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].0, 'J');
    assert_eq!(s[0].1, 1);
    assert_eq!(s[0].3, 0);
    // JS content preserves inner spacing and is not trimmed
    assert_eq!(s[0].2, " let a = 1; ");
}

#[test]
fn parse_expression_block() {
    let input = "<?= 1 + 2 ?>";
    let mut p = Parser::new(input);
    let res = p.parse();
    let s = collect_summaries(res.blocks);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].0, 'E');
    assert_eq!(s[0].1, 1);
    assert_eq!(s[0].3, 0);
    // Expression content is trimmed and excludes the '='
    assert_eq!(s[0].2, "1 + 2");
}

#[test]
fn parse_mixed_blocks() {
    let input = concat!(
        "<div>\n",               // HTML block 1 (line 1)
        "<? let x = 42; ?>",     // JS block (line 2)
        "\nP: <?= x ?>\n</div>"  // HTML block 2 (line 2)
    );

    let mut p = Parser::new(input);
    let res = p.parse();
    let s = collect_summaries(res.blocks);

    assert_eq!(s.len(), 5);
    // html 1
    assert_eq!(s[0].0, 'H');
    assert_eq!(s[0].1, 1);
    assert_eq!(s[0].3, 0);

    // js
    assert_eq!(s[1].0, 'J');
    assert_eq!(s[1].1, 2);
    assert_eq!(s[1].3, 0);

    // html 2 prefix up to expression (starts on line 2 after JS)
    assert_eq!(s[2].0, 'H');
    assert_eq!(s[2].1, 2);

    // epxr block starts on the next line
    assert_eq!(s[3].0, 'E');
    assert_eq!(s[3].1, 3);

    // trailing html after expression starts at line 3
    assert_eq!(s[4].0, 'H');
    assert_eq!(s[4].1, 3);
}

#[test]
fn nesting_levels_across_blocks() {
    let input = concat!(
        "<? if (cond) { ?>\n", // js open, ends with '{' -> level increments after this block
        "inside\n",            // html at nesting level 1
        "<? } ?>"              // js close, starts with '}' -> level decremented before this block
    );

    let mut p = Parser::new(input);
    let res = p.parse();
    let s = collect_summaries(res.blocks);

    assert_eq!(s.len(), 3);

    // first js block captured at level 0, then nesting becomes 1
    assert_eq!(s[0].0, 'J');
    assert_eq!(s[0].3, 0);

    // html is at level 1 due to the opened brace
    assert_eq!(s[1].0, 'H');
    assert_eq!(s[1].3, 1);

    // closing JS starts with '}', so its recorded level goes back to 0
    assert_eq!(s[2].0, 'J');
    assert_eq!(s[2].3, 0);
}

#[test]
fn blocks_to_js_emits_expected_code() {
    let input = "Hello <?= name ?>!\n<? log(name); ?>";
    let mut p = Parser::new(input);
    let res = p.parse();

    let js = blocks_to_js(res.blocks);

    // echo("Hello "), Expression -> echo(String(name)); echo("!\n"), js("log(name);")
    let expected_lines = vec![
        "echo(`Hello `);",
        "echo(String(name));",
        "echo(`!",
        "`);",
        "log(name);",
    ];

    let actual_lines: Vec<&str> = js.lines().collect();
    assert_eq!(actual_lines, expected_lines);
}

#[test]
fn set_content_resets_state() {
    let mut p = Parser::new("<? if (x) { ?>X<? } ?>");
    let _ = p.parse();

    // switch to plain html and ensure level is 0 for first block
    p.set_content("<p>y</p>");
    let res2 = p.parse();
    let s = collect_summaries(res2.blocks);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].0, 'H');
    assert_eq!(s[0].3, 0);
}
