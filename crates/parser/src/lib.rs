#[derive(Debug)]
pub struct CodeBlockContent {
    pub lineno: usize,
    pub colno: usize,
    pub content: String,
    pub level: usize,
}

#[derive(Debug)]
pub enum CodeBlock {
    Html(CodeBlockContent),
    Javascript(CodeBlockContent),
    Expression(CodeBlockContent),
}

#[derive(Default, Debug)]
pub struct ParseResults {
    pub blocks: Vec<Box<CodeBlock>>,
}

impl ParseResults {
    fn add_block(&mut self, block: Box<CodeBlock>) {
        self.blocks.push(block);
    }
}

pub struct Parser<'a> {
    content: &'a str,
    pos: usize,
    line: usize,
    nesting: usize,
}

impl<'a> Parser<'a> {
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            pos: 0,
            line: 1,
            nesting: 0,
        }
    }

    pub fn parse(&mut self) -> ParseResults {
        self.pos = 0;
        self.line = 1;
        self.nesting = 0;

        let mut results = ParseResults::default();
        while self.pos < self.content.len() {
            if self.lookahead("<?") {
                results.add_block(Box::new(self.parse_js_block()));
            } else {
                results.add_block(Box::new(self.parse_html_block()));
            }
        }
        results
    }

    pub fn set_content(&mut self, content: &'a str) {
        self.content = content;
        self.pos = 0;
        self.line = 1;
        self.nesting = 0;
    }

    fn parse_html_block(&mut self) -> CodeBlock {
        let start_line = self.line;
        let start_col = self.column_at(self.pos);
        let mut buf = String::new();

        while self.pos < self.content.len() && !self.lookahead("<?") {
            let c = self.consume();
            if c == '\n' {
                self.line += 1;
            }
            // HTML-encode JS-breaking characters so they don't break echo(`...`) literals
            match c {
                '\'' => buf.push_str("&#39;"), // single quote
                '"' => buf.push_str("&quot;"), // double quote
                '`' => buf.push_str("&#96;"),  // backtick
                _ => buf.push(c),
            }
        }

        CodeBlock::Html(CodeBlockContent {
            lineno: start_line,
            colno: start_col,
            content: buf,
            level: self.nesting,
        })
    }

    fn parse_js_block(&mut self) -> CodeBlock {
        let start_line = self.line;
        // opening "<?"
        let tag_pos = self.pos; // position of '<'
        let _ = self.consume(); // <
        let _ = self.consume(); // ?

        // 1-based column index.
        let mut start_col = self.column_at(tag_pos) + 2;

        let mut buf = String::new();
        while self.pos < self.content.len() && !self.lookahead("?>") {
            let c = self.consume();
            if c == '\n' {
                self.line += 1;
            }
            buf.push(c);
        }

        if self.lookahead("?>") {
            let _ = self.consume(); // ?
            let _ = self.consume(); // >
        }

        let trimmed_start = buf.trim_start();
        let trimmed_end = buf.trim_end();
        if trimmed_start.starts_with('}') {
            self.nesting = self.nesting.saturating_sub(1);
        }

        let level = self.nesting;

        if trimmed_end.ends_with('{') {
            self.nesting += 1;
        }

        // expression block if it starts with '=' after leading whitespace
        if trimmed_start.starts_with('=') {
            // find '=' in the original buffer to compute accurate expression column start.
            let eq_byte_idx = buf.find('=');
            let after_eq = trimmed_start[1..].trim();
            if let Some(eq_idx) = eq_byte_idx {
                // count chars from start of buf to '=' and whitespace after '=' to the first expr char
                let chars_to_eq = buf[..eq_idx].chars().count();
                let ws_after_eq = buf[eq_idx + '='.len_utf8()..]
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .count();
                start_col += chars_to_eq + 1 /* '=' */ + ws_after_eq;
            }
            CodeBlock::Expression(CodeBlockContent {
                lineno: start_line,
                colno: start_col,
                content: after_eq.to_string(),
                level,
            })
        } else {
            CodeBlock::Javascript(CodeBlockContent {
                lineno: start_line,
                colno: start_col,
                content: buf,
                level,
            })
        }
    }

    fn lookahead(&self, pat: &str) -> bool {
        let bytes = self.content.as_bytes();
        let pat_bytes = pat.as_bytes();
        if self.pos + pat_bytes.len() > bytes.len() {
            return false;
        }
        &bytes[self.pos..self.pos + pat_bytes.len()] == pat_bytes
    }

    fn consume(&mut self) -> char {
        let s = &self.content[self.pos..];
        let mut iter = s.chars();
        let ch = iter.next().unwrap_or('\0');
        // advance by the byte length of this char
        self.pos += ch.len_utf8();
        ch
    }
}

impl<'a> Parser<'a> {
    /// Compute the 1-based column number at the given byte position in `self.content`.
    /// Counts Unicode scalar values to avoid byte/char mismatches.
    fn column_at(&self, byte_pos: usize) -> usize {
        let prefix = &self.content[..byte_pos];
        match prefix.rfind('\n') {
            Some(nl_idx) => self.content[nl_idx + 1..byte_pos].chars().count() + 1,
            None => prefix.chars().count() + 1,
        }
    }
}

/// Convert parsed JHP blocks into executable JavaScript source.
pub fn blocks_to_js<I>(blocks: I) -> String
where
    I: IntoIterator<Item = Box<CodeBlock>>,
{
    let mut js_lines: Vec<String> = Vec::new();

    for block in blocks {
        match *block {
            CodeBlock::Javascript(block) => {
                js_lines.push(block.content.trim().into());
            }
            CodeBlock::Html(block) => {
                js_lines.push(format!("echo(`{}`);", block.content));
            }
            CodeBlock::Expression(block) => {
                js_lines.push(format!("echo(String({}));", block.content.trim()));
            }
        }
    }

    js_lines.join("\n")
}
